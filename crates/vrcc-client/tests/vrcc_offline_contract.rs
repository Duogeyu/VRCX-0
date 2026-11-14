// SPDX-License-Identifier: MPL-2.0

//! 不联网的契约测试：用官方文档的示例回答喂契约。
//!
//! 示例来自星遇同频开放接口文档仓库的 `examples/responses.json`（离线生成的
//! 假数据，MIT）。每个接口测三种形状：
//!
//! - **旧服务器（1.8.x）**：没有 1.11.0 以后加的字段；
//! - **现在（1.15.3）**：新字段都有，弃用中的字段也还在；
//! - **弃用字段停了以后**（2026-11-26 / 2026-12-26）：`vibe`、`can`、`need` 等不再返回。
//!
//! 1.15.3 把星轨相交的文档对上了线上：`stars[].at`、`cards[].seen` 不再返回，
//! `pair` 写进文档。所以这两处仍然按「旧服务器可能给、也可能不给」解析。
//!
//! 三种都要能解析。官方的规则是「v1 只做加法、弃用字段至少留 60 天」，
//! 这里钉住的就是客户端这一侧：新字段当可选、不依赖弃用字段。

use vrcx_0_vrcc_contracts::{
    VrccAnnouncements, VrccBoard, VrccErrorBody, VrccModelBoard, VrccModelDetail, VrccMutuals,
    VrccMyMap, VrccNotes, VrccRecentNotifications, VrccToday, VrccTokenResponse, VrccUnread,
};

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).unwrap_or_else(|error| panic!("should parse: {error}\n{json}"))
}

#[test]
fn boards_parse_old_new_and_week_shapes() {
    // 1.8.x：没有 `sort`。
    let old: VrccBoard = parse(
        r#"{"board":"maps","min":1,"maps":[{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1","name":"星夜茶屋","author":"某位作者","avg":8.5,"raters":2,"rank":1,"weight":"mid","page":"https://vrcc.cc/maps/wrld_aaaaaaaa-0000-4000-8000-0000000000a1","img":"https://vrcc.cc/maps/wrld_aaaaaaaa-0000-4000-8000-0000000000a1.jpg"}],"at":1793038216765}"#,
    );
    assert_eq!(old.sort, None);
    assert_eq!(old.maps[0].week, None);

    // 1.13.0 本周榜：多 `sort` 和每张的 `week`。
    let week: VrccBoard = parse(
        r#"{"board":"maps","sort":"week","min":1,"maps":[{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1","name":"星夜茶屋","author":"某位作者","hot":{"text":"光影很舒服","by":"某位彩蛋猎人","up":0,"went":false},"avg":8.5,"raters":2,"rank":1,"weight":"mid","page":"https://vrcc.cc/maps/wrld_aaaaaaaa-0000-4000-8000-0000000000a1","img":"https://vrcc.cc/maps/wrld_aaaaaaaa-0000-4000-8000-0000000000a1.jpg","week":2}],"at":1793038216765}"#,
    );
    assert_eq!(week.sort.as_deref(), Some("week"));
    assert_eq!(week.maps[0].week, Some(2));

    // 本周榜只按本周的打分排，`rank` 照旧是总榜名次：列表第一张可以是总榜第 7。
    let reordered: VrccBoard = parse(
        r#"{"board":"maps","sort":"week","min":1,"maps":[{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a7","rank":7,"week":5},{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1","rank":1,"week":2}],"at":1793038216765}"#,
    );
    assert_eq!(
        reordered
            .maps
            .iter()
            .map(|map| map.rank)
            .collect::<Vec<_>>(),
        vec![Some(7), Some(1)]
    );
    assert_eq!(
        week.maps[0]
            .hot
            .as_ref()
            .and_then(|hot| hot.text.as_deref()),
        Some("光影很舒服")
    );

    // 新上榜：还没人评分的图 `rank`、`avg` 是 `null`。
    let new: VrccBoard = parse(
        r#"{"board":"maps","sort":"new","min":1,"maps":[{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a2","name":"新图","author":null,"hot":null,"avg":null,"raters":0,"rank":null,"weight":null,"page":"https://vrcc.cc/maps/wrld_aaaaaaaa-0000-4000-8000-0000000000a2","img":null}],"at":1793038216767}"#,
    );
    assert_eq!(new.maps[0].rank, None);
    assert_eq!(new.maps[0].avg, None);
}

#[test]
fn my_map_parses_with_and_without_the_deprecated_fields() {
    // 1.8.x：有 `can` / `need`，没有 `imgUrl`。
    let old: VrccMyMap = parse(
        r#"{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1","name":"星夜茶屋","board":true,"pending":false,"off":false,"avg":8.5,"raters":2,"mine":9,"can":true,"need":null,"words":"光影很舒服","wordsOk":"ok","img":"/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1"}"#,
    );
    assert_eq!(old.mine, Some(9));
    assert_eq!(
        old.image_url().as_deref(),
        Some("https://vrcc.cc/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1")
    );

    // 1.14.0：多 `imgUrl`，`can` / `need` 还在（弃用中）。
    let now: VrccMyMap = parse(
        r#"{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1","name":"星夜茶屋","board":true,"pending":false,"off":false,"avg":8.5,"raters":2,"mine":9,"can":true,"need":null,"words":"光影很舒服","wordsOk":"ok","img":"/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1","imgUrl":"https://vrcc.cc/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1"}"#,
    );
    assert_eq!(
        now.img_url.as_deref(),
        Some("https://vrcc.cc/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1")
    );

    // 2026-12-26 以后：`can` / `need` 不再返回。
    let after: VrccMyMap = parse(
        r#"{"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1","name":"星夜茶屋","board":true,"pending":false,"off":false,"avg":8.5,"raters":2,"mine":null,"words":null,"wordsOk":null,"img":"/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1","imgUrl":"https://vrcc.cc/api/match?xdimg=wrld_aaaaaaaa-0000-4000-8000-0000000000a1"}"#,
    );
    assert!(after.is_visible());
    assert_eq!(after.mine, None);
}

#[test]
fn mutuals_parse_rate_when_given_and_when_not() {
    let now: VrccMutuals = parse(
        r#"{"stars":[{"n":2,"name":"彩蛋猎人","group":"G","star":102,"vrc":"Player2","vrcUser":null,"at":1792865416696,"rate":{"played":true,"mine":4}},{"n":3,"name":"拾光旅人","group":"L","star":null,"vrc":"Player3","vrcUser":"usr_00000000-0000-4000-8000-000000000003","at":1792865416696,"rate":{"played":null,"mine":4.5}},{"n":4,"name":"夜航","group":"W","star":null,"vrc":"Player4","vrcUser":null,"at":1793038216696,"rate":null}],"n":3}"#,
    );
    let rate = now.stars[0].rate.as_ref().expect("rate");
    assert_eq!(rate.played, Some(true));
    assert_eq!(rate.mine, Some(4.0));
    // 半颗星一档。
    assert_eq!(
        now.stars[1].rate.as_ref().and_then(|rate| rate.mine),
        Some(4.5)
    );
    assert_eq!(
        now.stars[1].rate.as_ref().and_then(|rate| rate.played),
        None
    );
    // 连上当天是 `null`。
    assert!(now.stars[2].rate.is_none());

    // 1.11.0 以前没有 `rate`。
    let old: VrccMutuals = parse(
        r#"{"stars":[{"n":2,"name":"彩蛋猎人","group":"G","star":102,"vrc":"Player2","vrcUser":null,"at":1792865416696}],"n":1}"#,
    );
    assert!(old.stars[0].rate.is_none());
}

#[test]
fn star_cards_parse_with_and_without_vibe() {
    let card = r#"{"n":3,"pos":"G1","name":"彩蛋猎人","line":"进了新世界先去找隐藏房间。","group":"G","groupName":"漫游星群",VIBE"intro":"喜欢在安静的世界里散步。","about":[],"common":["你们都在漫游星群"],"photo":false,"rating":null,"star":null,"verified":false,"scores":{"G":50,"W":50,"Z":50,"L":50},"deco":null,"sprout":true,"why":"cross","linked":false,"mutual":false}"#;
    let round =
        r#""round":{"at":1793038216809,"next":1793117416809,"used":0,"max":6,"remind":false}"#;
    // 现在带 `vibe`，2026-12-26 以后不带。
    for vibe in [r#""vibe":"随场","#, ""] {
        let json = format!(
            r##"{{"day":"#1","cards":[{}],{round}}}"##,
            card.replace("VIBE", vibe)
        );
        let today: VrccToday = parse(&json);
        assert_eq!(today.cards.len(), 1);
        assert_eq!(today.cards[0].group_name.as_deref(), Some("漫游星群"));
        assert_eq!(today.round.max, Some(6));
    }
}

#[test]
fn unread_notes_and_announcements_parse() {
    let unread: VrccUnread = parse(r#"{"inbox":2,"cross":1,"chats":1,"notes":2}"#);
    assert_eq!(unread.cross, Some(1));
    assert_eq!(unread.total(), 6);
    // 令牌没有 `crossings`（比如 `dev-` 标识）：服务器不给 `cross` 这个键，
    // 要和「有、但是 0」分开。
    let no_cross: VrccUnread = parse(r#"{"inbox":2,"chats":1,"notes":2}"#);
    assert_eq!(no_cross.cross, None);
    assert_eq!(no_cross.total(), 5);
    // 以后多一处红点也不该坏。
    let more: VrccUnread = parse(r#"{"inbox":0,"cross":0,"chats":0,"notes":0,"later":9}"#);
    assert_eq!(more.total(), 0);

    let notes: VrccNotes = parse(
        r#"{"notes":[{"id":3,"kind":"map","text":"你刚去了「星夜茶馆」，给它打个分吧。","at":1793038216696,"seen":true,"post":null,"world":"wrld_aaaaaaaa-0000-4000-8000-0000000000a1"},{"id":2,"kind":"yq","text":"有颗星在你的星友圈动态下留了言。","at":1793038216696,"seen":false,"post":1,"world":null},{"id":1,"kind":"gift","text":"一颗互相连上的星送了你一件礼物，在「我的仓库」里。","at":1793038216696,"seen":false,"post":null,"world":null}]}"#,
    );
    assert_eq!(notes.notes.len(), 3);
    assert_eq!(
        notes.notes[0].world.as_deref(),
        Some("wrld_aaaaaaaa-0000-4000-8000-0000000000a1")
    );
    assert_eq!(notes.notes[1].post, Some(1));
    assert_eq!(notes.notes[2].seen, Some(false));

    let open: VrccAnnouncements = parse(
        r#"{"stop":null,"posts":[{"id":1,"level":"pop","title":"地图榜开放打分","body":"去过的地图都可以打 1–10 分。","link":"https://vrcc.cc/match/?tab=maps","cover":"https://vrcc.cc/assets/posts/maps.webp","end":1793643016696}]}"#,
    );
    assert!(open.stop.is_none());
    assert_eq!(open.posts[0].level.as_deref(), Some("pop"));

    let stopped: VrccAnnouncements =
        parse(r#"{"stop":{"say":"升级数据库","until":null},"posts":[]}"#);
    assert_eq!(
        stopped.stop.as_ref().and_then(|stop| stop.say.as_deref()),
        Some("升级数据库")
    );
}

#[test]
fn recent_and_token_parse() {
    let recent: VrccRecentNotifications = parse(
        r#"{"items":[{"id":1,"title":"星遇同频","body":"有颗星连了你。","url":"/match/?tab=inbox","at":1793038216696}],"read":0}"#,
    );
    assert_eq!(
        recent.items[0].absolute_url(),
        "https://vrcc.cc/match/?tab=inbox"
    );

    let token: VrccTokenResponse = parse(
        r#"{"access_token":"31e7abc6884855a29b82f7191a684590d65f98d32befeea7","token_type":"Bearer","scope":"profile notifications maps","me":{"star":101,"vrc":null}}"#,
    );
    assert_eq!(token.me.and_then(|me| me.star), Some(101));
}

#[test]
fn error_bodies_carry_stop_and_every() {
    let stop: VrccErrorBody =
        parse(r#"{"error":"stop","say":"升级数据库，约半小时","until":1793040000000}"#);
    assert_eq!(stop.error.as_deref(), Some("stop"));
    assert_eq!(stop.until, Some(1_793_040_000_000));

    let soon: VrccErrorBody = parse(r#"{"error":"too_soon","every":60}"#);
    assert_eq!(soon.every, Some(60));
}

#[test]
fn a_malformed_hot_comment_does_not_break_the_whole_board() {
    // `hot` 不在官方 openapi 的 `BoardMap` 里，是 1.10.1 才写进文档的附加字段，
    // 随时可能改形状或下线。它坏掉时**只能少显示一句短评**，不能让整份
    // 地图榜解析失败——那会让榜单整页空白、腕表名次也不再更新。
    let board: VrccBoard = parse(
        r#"{"board":"maps","min":1,"sort":"top","maps":[
            {"world":"wrld_b1","name":"形状变了","avg":8.5,"raters":2,"rank":1,"hot":{"text":123,"by":["a"],"up":"7","went":"yes"}},
            {"world":"wrld_b2","name":"正常","avg":9.0,"raters":3,"rank":2,"hot":{"text":"很棒","by":"某位倾听者","up":9,"went":true}},
            {"world":"wrld_b3","name":"没有热评","avg":7.0,"raters":1,"rank":3}
        ],"at":1793038216765}"#,
    );

    // 三个条目都要在：形状坏掉只影响它自己那一句短评。
    assert_eq!(board.maps.len(), 3);

    // 数字被当成字符串读出来；确实是字符串的才留。
    let broken = board.maps[0].hot.as_ref().expect("hot exists");
    assert_eq!(broken.text.as_deref(), Some("123"));
    assert_eq!(broken.by, None, "数组不是字符串，当作没有");
    assert_eq!(broken.up, Some(7), "字符串里的数字能读出来");
    assert_eq!(broken.went, None, "\"yes\" 不是 bool，当作没有");

    let normal = board.maps[1].hot.as_ref().expect("hot exists");
    assert_eq!(normal.text.as_deref(), Some("很棒"));
    assert_eq!(normal.went, Some(true));

    assert!(board.maps[2].hot.is_none());
}

/// 星轨相交：列表要能解析，且 `why` 的七种取值都要分类正确（外加未知值）。
///
/// `why` 的分类**直接影响界面行为**：`hide` 要求客户端把整个模块藏起来
/// （16–17 岁星池、或玩家在隐私设置里关了星轨相交），而 `verify` / `wait`
/// 只是「还没准备好」，要显示原因而不是隐藏。两者搞反的后果不对称——
/// 把 `hide` 当 `verify` 会向不该看到的人暴露「这里有星轨相交」这个事实。
#[test]
fn crossings_parse_and_the_why_reasons_classify_correctly() {
    use vrcx_0_vrcc_contracts::{VrccCrossingWhy, VrccCrossings};

    // 正常一条：有擦肩记录，还没连线。
    // `group` 用官方枚举串（`G`/`W`/`Z`/`L`/`A`），`me` 用官方 `{star, vrc}`。
    let list: VrccCrossings = parse(
        r#"{"stars":[
            {"n":77,"name":"拾光旅人","line":"放慢脚步逛世界","group":"G","star":31,
             "intro":"平时喜欢在安静的世界拍照、听音乐","vrc":null,
             "linked":false,"mutual":false,"at":1792830000000,"fresh":true},
            {"n":78,"name":"已连上的","group":"W","vrc":"StarTraveler",
             "linked":true,"mutual":true,"at":1792830000001,"fresh":false}
        ],"n":2,"why":null,"me":{"star":1024,"vrc":"夜航星"}}"#,
    );

    assert_eq!(list.stars.len(), 2);
    assert_eq!(list.why, None);

    let first = &list.stars[0];
    assert_eq!(first.n, 77);
    assert_eq!(first.name.as_deref(), Some("拾光旅人"));
    assert_eq!(first.line.as_deref(), Some("放慢脚步逛世界"));
    assert_eq!(
        first.intro.as_deref(),
        Some("平时喜欢在安静的世界拍照、听音乐")
    );
    assert!(!first.linked && !first.mutual);
    assert!(first.fresh);
    // 未双向连线时 `vrc` 是 null——这是「连上才解锁」的信息。
    assert!(first.vrc.is_none());

    let second = &list.stars[1];
    assert!(second.linked && second.mutual);
    assert_eq!(second.vrc.as_deref(), Some("StarTraveler"));

    // `me` 是官方的 `{star, vrc}`：点亮编号 + VRChat 名字。
    assert_eq!(list.me.as_ref().and_then(|me| me.star), Some(1024));
    assert_eq!(
        list.me.as_ref().and_then(|me| me.vrc.as_deref()),
        Some("夜航星")
    );

    // `group` 照官方枚举原样保留，不做映射。
    assert_eq!(first.group.as_deref(), Some("G"));
    assert_eq!(second.group.as_deref(), Some("W"));

    // 七种 why 取值 + 未知值 + 缺省值。
    assert_eq!(VrccCrossingWhy::parse(None), VrccCrossingWhy::Available);
    assert_eq!(VrccCrossingWhy::parse(Some("")), VrccCrossingWhy::Available);
    assert_eq!(
        VrccCrossingWhy::parse(Some("hide")),
        VrccCrossingWhy::Hidden
    );
    assert_eq!(
        VrccCrossingWhy::parse(Some("verify")),
        VrccCrossingWhy::Unverified
    );
    assert_eq!(
        VrccCrossingWhy::parse(Some("wait")),
        VrccCrossingWhy::WaitingVerify
    );
    assert_eq!(
        VrccCrossingWhy::parse(Some("queued")),
        VrccCrossingWhy::Queued
    );
    assert_eq!(VrccCrossingWhy::parse(Some("sent")), VrccCrossingWhy::Sent);
    assert_eq!(VrccCrossingWhy::parse(Some("gone")), VrccCrossingWhy::Gone);
    assert_eq!(
        VrccCrossingWhy::parse(Some("friend")),
        VrccCrossingWhy::NotFriend
    );
    // 服务器以后加的新原因：认得成 Unknown，但不能解析失败。
    assert_eq!(
        VrccCrossingWhy::parse(Some("something-new")),
        VrccCrossingWhy::Unknown
    );

    // 只有 `hide` 要求整个模块隐藏。判定收在一处（`is_hidden`），界面只看
    // `hidden` 字段，不再各自去比字符串。
    assert!(VrccCrossingWhy::Hidden.is_hidden());
    for reason in [
        VrccCrossingWhy::Available,
        VrccCrossingWhy::Unverified,
        VrccCrossingWhy::WaitingVerify,
        VrccCrossingWhy::Queued,
        VrccCrossingWhy::Sent,
        VrccCrossingWhy::Gone,
        VrccCrossingWhy::NotFriend,
        VrccCrossingWhy::Unknown,
    ] {
        assert!(!reason.is_hidden(), "{reason:?} should stay visible");
    }

    // 解析出来的结构：默认不隐藏，`hide` 时由应用层置上 `hidden`。
    let mut list: VrccCrossings = parse(r#"{"stars":[],"n":0,"why":"hide"}"#);
    assert!(!list.hidden, "契约层不替服务器做判断，默认 false");
    list.settle_visibility();
    assert!(list.hidden);

    let mut list: VrccCrossings = parse(r#"{"stars":[],"n":0,"why":"verify"}"#);
    list.settle_visibility();
    assert!(!list.hidden, "`verify` 只是暂时不可用，不能藏起来");

    // 旧服务器/空列表也要能解析（字段全缺省）。
    let empty: VrccCrossings = parse(r#"{"stars":[],"n":0,"why":null,"me":null}"#);
    assert!(empty.stars.is_empty());
    assert!(empty.me.is_none());
    assert!(!empty.hidden);
}

/// `/me/crossings/cards` 的星卡：`linkedMe` / `at` / `why` 这三个字段不在官方
/// `StarCard` 的字段表里，只在示例里出现（`additionalProperties: true` 的那种）。
///
/// 三个都关系到界面上能说清什么：`at` 是「最近一次擦肩是什么时候」，`linkedMe`
/// 是「TA 是不是已经连向我」，`why` 是「这张卡为什么出现在这一页」。少一个，
/// 界面就只能说出「有这个人」，其余全靠猜。
#[test]
fn crossing_cards_keep_the_fields_the_star_card_table_never_lists() {
    use vrcx_0_vrcc_contracts::VrccCrossingCards;

    let cards: VrccCrossingCards = parse(
        r#"{"cards":[
            {"n":3,"name":"彩蛋猎人","group":"G","why":"cross","linked":false,
             "mutual":false,"linkedMe":true,"at":1792830000000},
            {"n":4,"name":"夜航"}
        ],"seen":0,"left":5,"max":5}"#,
    );

    assert_eq!(cards.cards.len(), 2);
    let first = &cards.cards[0];
    assert_eq!(first.linked_me, Some(true));
    assert_eq!(first.at, Some(1_792_830_000_000));
    assert_eq!(first.why.as_deref(), Some("cross"));
    // 和 `linked` 是两条独立的边：TA 连向我，不代表我连了 TA。
    assert_eq!(first.linked, Some(false));
    assert_eq!(first.mutual, Some(false));

    // 没有这三个键的星卡照旧能读——它们不是必填。
    let second = &cards.cards[1];
    assert_eq!(second.linked_me, None);
    assert_eq!(second.at, None);
    assert_eq!(second.why, None);

    assert_eq!(cards.seen, Some(0));
    assert_eq!(cards.left, Some(5));
    assert_eq!(cards.max, Some(5));
}

/// `/me/crossings/cards` 的 `pair`：网页「星轨相交」点开一颗星时，浮层上那几行
/// 全靠它（什么时候互连、从哪认识、在哪遇见）。
///
/// 这个键当初不在官方文档、`StarCard` 字段表、`examples/responses.json` 里，
/// 是我们实测出来、再反馈到上游的；1.15.3 起官方已经写进文档，并明确：
/// 只有 `mutual: true` 的卡才带它，`met.at` 只精确到天。
///
/// 契约这边仍然全部按可选解析——没连上的卡没有这个键，老服务器也没有；
/// `met.world` 必须是**世界名原文**：有的世界名自带「｜社群名」，有的没有，
/// 按分隔符拆会把正常世界名拆坏。
#[test]
fn crossing_cards_carry_the_pair_section_the_docs_never_mention() {
    use vrcx_0_vrcc_contracts::VrccCrossingCards;

    let cards: VrccCrossingCards = parse(
        r#"{"cards":[
            {"n":827,"pos":"L2","pair":{"at":1792778779575,"src":"yq","mine":false,
             "met":{"at":1792900800000,"world":"天井 Skywell｜中文社区 Chinese Community"}}},
            {"n":859,"pos":"Z0","pair":{"at":1792925092206,"src":"fof","mine":true,
             "met":{"at":1792900800000,"world":"咖啡厅v1․31"}}},
            {"n":4,"name":"没有 pair 的旧卡"}
        ],"left":5,"max":5}"#,
    );

    assert_eq!(cards.cards.len(), 3);

    let with_club = &cards.cards[0];
    assert_eq!(with_club.pos.as_deref(), Some("L2"));
    let pair = with_club.pair.as_ref().expect("pair 必须解析出来");
    assert_eq!(pair.at, Some(1_792_778_779_575));
    assert_eq!(pair.src.as_deref(), Some("yq"));
    assert_eq!(pair.mine, Some(false));
    let met = pair.met.as_ref().expect("met 必须解析出来");
    assert_eq!(met.at, Some(1_792_900_800_000));
    // 世界名原样保留，连全角分隔符一起：这不是「世界 + 社群」的拼接串。
    assert_eq!(
        met.world.as_deref(),
        Some("天井 Skywell｜中文社区 Chinese Community")
    );

    // 没有分隔符的普通世界名，同样原样保留（它里面还有 U+2024）。
    let plain = &cards.cards[1];
    let world = plain
        .pair
        .as_ref()
        .and_then(|pair| pair.met.as_ref())
        .and_then(|met| met.world.as_deref());
    assert_eq!(world, Some("咖啡厅v1․31"));
    assert_eq!(
        plain.pair.as_ref().and_then(|pair| pair.src.as_deref()),
        Some("fof")
    );

    // 没有 `pair` 的卡照旧能读：它不是必填，界面会整块不渲染。
    assert!(cards.cards[2].pair.is_none());
}

/// 模型榜（1.17.0 起）：`GET /boards/models`。
///
/// 官方 `BoardModel` 的必填字段是 `id` / `name` / `short` / `shop` / `price` /
/// `likes` / `week` / `rank` / `page` / `img` / `booth` / `hot`，其中除了
/// `id` / `name` / `likes` 之外都允许 `null`。`place` / `move` / `fresh` 只在
/// `sort=week` 时有。
#[test]
fn models_board_parses_top_week_and_new_shapes() {
    // 总榜：没有 `place` / `move` / `fresh`。
    let top: VrccModelBoard = parse(
        r#"{"board":"models","sort":"top","total":1,"next":null,"at":1792830000000,"models":[{"id":8962830,"name":"オリジナル3Dモデル「星」","short":"星","shop":"星屋","price":"¥1,500","likes":42,"week":7,"rank":1,"page":"https://vrcc.cc/models/8962830","img":"https://booth.pximg.net/example.jpg","booth":"https://booth.pm/ja/items/8962830","hot":{"text":"表情很多，换装方便","by":"某位彩蛋猎人","up":3}}]}"#,
    );
    assert_eq!(top.sort.as_deref(), Some("top"));
    assert_eq!(top.total, Some(1));
    assert_eq!(top.next, None);
    assert_eq!(top.models.len(), 1);

    let model = &top.models[0];
    assert_eq!(model.id, 8962830);
    assert_eq!(model.likes, 42);
    assert_eq!(model.rank, Some(1));
    assert_eq!(model.display_name(), "星");
    assert_eq!(
        model.hot.as_ref().and_then(|hot| hot.text.as_deref()),
        Some("表情很多，换装方便")
    );
    // 总榜上这三个字段不该有值。
    assert_eq!(model.place, None);
    assert_eq!(model.r#move, None);
    assert_eq!(model.fresh, None);

    // 本周榜：带 `place` / `move` / `fresh`。`move` 是 Rust 关键字，
    // 靠 `#[serde(rename = "move")]` 才能读到——漏掉 rename 时这里会是 None。
    let week: VrccModelBoard = parse(
        r#"{"board":"models","sort":"week","weeks":[{"w":2965,"from":1793548800000}],"at":1792830000000,"models":[{"id":8962830,"name":"星","short":null,"shop":null,"price":null,"likes":42,"week":2,"rank":1,"page":"https://vrcc.cc/models/8962830","img":"https://booth.pximg.net/example.jpg","booth":"https://booth.pm/ja/items/8962830","hot":null,"place":1,"move":3,"fresh":true}]}"#,
    );
    assert_eq!(week.sort.as_deref(), Some("week"));
    assert_eq!(week.weeks.len(), 1);
    assert_eq!(week.weeks[0].w, Some(2965));
    let row = &week.models[0];
    assert_eq!(row.place, Some(1));
    assert_eq!(row.r#move, Some(3));
    assert_eq!(row.fresh, Some(true));
    // `short` 是 null 时退回 BOOTH 原文。
    assert_eq!(row.display_name(), "星");

    // 新上榜：还没人喜欢的模型 `rank` 是 null。
    let new: VrccModelBoard = parse(
        r#"{"board":"models","sort":"new","models":[{"id":123,"name":"新しいモデル","short":null,"shop":null,"price":null,"likes":0,"week":0,"rank":null,"page":"https://vrcc.cc/models/123","img":null,"booth":null,"hot":null}]}"#,
    );
    assert_eq!(new.models[0].rank, None);
    assert_eq!(new.models[0].likes, 0);

    // 旧服务器（1.17.0 之前）没有这个接口，回的是 `404 not_found`，不会有正文。
    // 但只要字段缺失，解析也不能炸：全空对象就是空榜。
    let bare: VrccModelBoard = parse(r#"{}"#);
    assert!(bare.models.is_empty());
    assert_eq!(bare.sort, None);
}

/// 模型榜条目上的附加字段要能容忍「形状不对」。
///
/// `hot` 和 `likes` 都不是契约必需的：一句坏短评、一个写成字符串的计数，
/// 都不该让整个榜单空白。
#[test]
fn models_board_tolerates_odd_field_shapes() {
    // `likes` 写成字符串（网页上就是这种写法）、`hot` 的字段类型也不标准。
    let odd: VrccModelBoard = parse(
        r#"{"board":"models","models":[{"id":1,"name":"モデル","likes":"42","hot":{"text":123,"by":null,"up":"7"}}]}"#,
    );
    let model = &odd.models[0];
    assert_eq!(model.likes, 42, "字符串计数要读成数字");
    // `text` 是数字：读成 "123"（lenient_optional_string 的本意就是不丢信息）。
    assert_eq!(
        model.hot.as_ref().and_then(|hot| hot.text.as_deref()),
        Some("123")
    );
    assert_eq!(model.hot.as_ref().and_then(|hot| hot.up), Some(7));

    // `likes` 完全缺失：读成 0，而不是解析失败。
    let missing: VrccModelBoard = parse(r#"{"models":[{"id":2,"name":"モデル"}]}"#);
    assert_eq!(missing.models[0].likes, 0);
    // 其它字段全缺省，`display_name` 仍然可用。
    assert_eq!(missing.models[0].display_name(), "モデル");
    assert_eq!(missing.models[0].short, None);
}

/// 单个模型（`GET /models/{id}`）：`model` + 最多 2 条热评。
#[test]
fn model_detail_parses_with_hots() {
    let detail: VrccModelDetail = parse(
        r#"{"model":{"id":8962830,"name":"オリジナル3Dモデル「星」","short":"星","shop":"星屋","price":"¥1,500","likes":42,"week":7,"rank":1,"page":"https://vrcc.cc/models/8962830","img":"https://booth.pximg.net/example.jpg","booth":"https://booth.pm/ja/items/8962830","hot":null},"hots":[{"text":"表情很多","by":"某位彩蛋猎人","up":3},{"text":"换装方便","by":"某位星语者","up":2}],"at":1792830000000}"#,
    );
    assert_eq!(detail.model.as_ref().map(|model| model.id), Some(8962830));
    assert_eq!(detail.hots.len(), 2);
    assert_eq!(detail.hots[0].up, Some(3));
    assert_eq!(detail.hots[1].by.as_deref(), Some("某位星语者"));

    // 不在榜上时服务器回 404，没有正文；空的回答也该能解析。
    let empty: VrccModelDetail = parse(r#"{}"#);
    assert!(empty.model.is_none());
    assert!(empty.hots.is_empty());
}
