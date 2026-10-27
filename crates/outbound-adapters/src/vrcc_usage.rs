//! 星遇同频日配额计数的本地存储。
//!
//! 为什么需要落盘：官方的日配额是**按天**算的（第一份读 1000 次 / `device/code`
//! 30 个），而计数只存在进程内存里的话，**重启即清零**——本地预扣形同虚设。
//! 玩家会看到「今日已用 0/1000」，服务器那边却已经扣过大半，直到撞上
//! `429 too_many` 才暴露，而那时只能等第二天。
//!
//! 服务器回答 `RateLimit-*`（1.11.0 起）以后，存进来的是服务器的准数；
//! 本地计数只在拿到下一次响应头之前兜底。
//!
//! 这里存的**不是凭据**（配额数字不含敏感信息），所以走普通 `configs` 明文，
//! 不需要 `secrets_at_rest`。令牌的加密在 `vrcc_token.rs` 里单独处理。

use std::sync::Arc;

use vrcx_0_application::vrcc::{VrccUsageRecord, VrccUsageStore};
use vrcx_0_persistence::{config, DatabaseService};

/// 日配额计数的配置键。
///
/// 存成 JSON：`{"date":"2026-10-26","personal":7,"deviceCode":1}`。
/// 用一个键而不是三个，保证读出来的是一次原子的快照，不会出现
/// 「日期是今天的、计数是昨天的」这种半更新状态。
const VRCC_USAGE_KEY: &str = "vrccDailyUsage";

/// 把日配额计数写进配置表。
#[derive(Clone)]
pub struct LocalVrccUsageStore {
    db: Arc<DatabaseService>,
}

impl LocalVrccUsageStore {
    pub fn new(db: Arc<DatabaseService>) -> Self {
        Self { db }
    }
}

impl VrccUsageStore for LocalVrccUsageStore {
    fn load(&self) -> Option<VrccUsageRecord> {
        let raw = config::get_string(&self.db, VRCC_USAGE_KEY, "").ok()?;
        if raw.trim().is_empty() {
            return None;
        }
        // 存的是 `{"date":…,"personal":…,"deviceCode":…}`，和
        // `integrations::vrcc::DailyUsage` 的序列化形状一致；这里改用本地
        // 解析，避免存档格式跟着网络层的类型走。
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Persisted {
            date: String,
            personal: i64,
            device_code: i64,
        }
        match serde_json::from_str::<Persisted>(&raw) {
            Ok(usage) => Some(VrccUsageRecord {
                date: usage.date,
                personal: usage.personal.max(0),
                device_code: usage.device_code.max(0),
            }),
            Err(error) => {
                // 存档坏了就当作没有：从 0 开始记。清掉坏数据，
                // 否则每次启动都会重复解析失败。
                tracing::warn!(%error, "vrcc: daily usage snapshot is corrupt, resetting");
                let _ = config::set_string(&self.db, VRCC_USAGE_KEY, "");
                None
            }
        }
    }

    fn save(&self, record: &VrccUsageRecord) {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Persisted<'a> {
            date: &'a str,
            personal: i64,
            device_code: i64,
        }
        let Ok(text) = serde_json::to_string(&Persisted {
            date: &record.date,
            personal: record.personal,
            device_code: record.device_code,
        }) else {
            tracing::warn!("vrcc: failed to serialize daily usage");
            return;
        };
        if let Err(error) = config::set_string(&self.db, VRCC_USAGE_KEY, &text) {
            // 写失败不该让请求失败：配额记不准比功能不可用轻得多。
            tracing::warn!(%error, "vrcc: failed to persist daily usage");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_key_is_stable() {
        // 键名变了等于旧计数失效，重启后又从头记——这条防止手滑改名。
        assert_eq!(VRCC_USAGE_KEY, "vrccDailyUsage");
    }

    #[test]
    fn daily_usage_persists_with_the_expected_shape() {
        // 存档字段名是 camelCase。改形状等于旧计数失效（重启后从头记），
        // 所以这里钉住线上真实的键名。
        let (db, _dir) = test_db();
        let store = LocalVrccUsageStore::new(db.clone());
        store.save(&VrccUsageRecord {
            date: "2026-10-26".to_string(),
            personal: 7,
            device_code: 1,
        });
        let raw = config::get_string(&db, VRCC_USAGE_KEY, "").expect("read back");
        assert!(raw.contains("\"date\":\"2026-10-26\""));
        assert!(raw.contains("\"personal\":7"));
        assert!(raw.contains("\"deviceCode\":1"));
    }

    #[test]
    fn daily_usage_round_trips() {
        let (db, _dir) = test_db();
        let store = LocalVrccUsageStore::new(db);
        let record = VrccUsageRecord {
            date: "2026-10-26".to_string(),
            personal: 42,
            device_code: 3,
        };
        store.save(&record);
        assert_eq!(store.load(), Some(record));
    }

    /// 建一个临时库。配额存储是唯一被测的对象，用真实 SQLite 比模拟更可信。
    fn test_db() -> (Arc<DatabaseService>, TempDir) {
        let dir = TempDir::new();
        let db = Arc::new(
            DatabaseService::new(&dir.path().join("VRCX-0.sqlite3")).expect("test database"),
        );
        (db, dir)
    }

    /// 用后即删的临时目录。
    ///
    /// 不引第三方 crate：只需要一个独占的目录 + Drop 时清理，
    /// `vrcc_token.rs` 的测试里也是同样做法。
    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir()
                .join(format!("vrcx0-vrcc-usage-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
