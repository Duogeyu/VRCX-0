// SPDX-License-Identifier: MPL-2.0

//! 星遇同频站外提醒在通知中心里的那几行。
//!
//! 只是把两条通知表查询包一层：宿主（`runtime-host-desktop`）不直接碰
//! persistence 的查询函数，见 `backend_architecture` 的
//! `desktop_runtime_host_reaches_infrastructure_only_through_listed_modules`。

use serde_json::Value;
use vrcx_0_persistence::DatabaseService;

use crate::Result;

/// 写一条站外提醒行。按 id 冲突合并，`seen` 取两边较大的那个。
pub fn upsert_vrcc_notice_row(db: &DatabaseService, owner: &str, row: Value) -> Result<()> {
    vrcx_0_persistence::notifications::notification_add_v2(db, owner.to_string(), row)
        .map_err(crate::Error::from)
}

/// 把 id 以 `id_prefix` 开头、编号不大于 `read` 的站外提醒行标成已读。
pub fn mark_vrcc_notice_rows_seen_up_to(
    db: &DatabaseService,
    owner: &str,
    id_prefix: &str,
    read: i64,
) -> Result<()> {
    vrcx_0_persistence::notifications::notification_mark_seen_up_to(
        db,
        owner.to_string(),
        id_prefix,
        read,
    )
    .map_err(crate::Error::from)
}
