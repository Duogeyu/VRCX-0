pub mod activity;
pub mod activity_page;
pub mod activity_view;
mod avatar;
pub mod background_image;
pub mod browse_history;
mod community_theme;
pub mod community_theme_protocol;
mod data_dir_migration;
pub mod database_maintenance;
mod database_upgrade;
mod entity_cache;
pub mod external_api;
mod favorites;
pub mod feed;
pub mod feed_live;
pub mod friend_log;
pub mod game_log;
pub mod game_log_query;
mod legacy_migration;
pub mod llm;
pub mod local_moderation;
mod media;
pub mod memos;
pub mod mutual_graph;
pub mod notifications;
mod persistence;
pub mod player_list;
mod profile_backup;
pub mod profile_bio;
mod profile_config;
pub mod realtime;
pub mod social_aggregates;
pub mod telemetry;
mod translation;
pub mod vrchat_api;
pub mod vrchat_requests;
mod web;
pub mod world_collections;

/// 星遇同频契约的再导出。
///
/// 类型本体住在独立 crate [`vrcx_0_vrcc_contracts`]，整包 MPL-2.0、依赖闭包
/// 里没有任何本仓库的 GPL crate，外部消费者可以单独取用。这里保留
/// `vrcx_0_contracts::vrcc::…` 这条路径，是因为仓库内已有几十处引用，
/// 一次性改写成新 crate 只会把这次搬家的 diff 撑大。
pub use vrcx_0_vrcc_contracts as vrcc;

pub use avatar::{
    AvatarTagInput, AvatarTagOutput, AvatarTagsPatchInput, AvatarTimeSpentOutput, AvatarUsageRow,
    AvatarWearSegment,
};
pub use community_theme::{
    CommunityThemeAuthor, CommunityThemeCatalog, CommunityThemeManifest, CommunityThemeStatsById,
    CommunityThemeStatsEntry,
};
pub use data_dir_migration::{
    DataDirCleanupPending, DataDirCleanupReport, DataDirMigrationResult,
    DataDirMigrationResultStatus, DataDirMigrationTargetState, DataDirMigrationWarning,
    DATA_DIR_MIGRATION_SPACE_MARGIN_BYTES,
};
pub use database_upgrade::DatabaseUpgradeStatus;
pub use entity_cache::{
    AvatarCacheOutput, CacheEntityInput, FileMetadataOutput, WorldSummaryOutput,
};
pub use favorites::{
    FavoriteRow, SavedGroupCollection, SavedGroupCollectionCreateInput,
    SavedGroupCollectionDeleteInput, SavedGroupFavoriteAddInput, SavedGroupFavoriteRemoveInput,
    SavedGroupFavoritesSnapshot, VrchatFavoriteType,
};
pub use legacy_migration::{
    LegacyMigrationPaths, LegacyMigrationProgress, LegacyVrcxDiscovery, LegacyVrcxMigrationStatus,
    LegacyVrcxSource,
};
pub use llm::{
    AssistantTurn, ChatMessage, FunctionCall, LlmEndpointDetectModelsResult, LlmModelReasoning,
    LlmRequestOptions, ToolCall, ToolDefinition,
};
pub use media::UgcCategory;
pub use persistence::{ApplicationErrorPayload, ApplicationErrorSource, SqliteErrorCategory};
pub use profile_backup::{
    ProfileBackupKind, ProfileRestoreAppVersionCheck, ProfileRestoreArchiveCheck,
    ProfileRestoreDataDisposition, ProfileRestoreDatabaseCheck, ProfileRestoreDatabaseVersionCheck,
    ProfileRestoreFailure, ProfileRestoreFailureCode, ProfileRestoreManifestSummary,
    ProfileRestoreResult, ProfileRestoreResultStatus, ProfileRestoreValidation,
    ProfileRestoreValidationOutcome,
};
pub use profile_config::{resolve_config_key, ConfigMutation, ConfigReadEntry, ConfigWriteEntry};
pub use translation::TranslationProvider;
pub use vrchat_api::{
    VrchatAuthFailureKind, VrchatFailure, VrchatJsonResponse, VrchatRequest, VrchatRequestBody,
    VrchatResponse, VrchatResponseClass, VrchatResponsePolicy, VrchatScope, VrchatUpload,
};
pub use vrcx_0_runtime_event::{
    runtime_event_payload, InstanceRosterMember, InstanceRosterObserver, InstanceRosterSnapshot,
    RuntimeEventPayload,
};
pub use web::{
    RealtimeAuthTokenFetch, RealtimeConnectionOptions, WebExecuteRequest, WebMultipartPart,
    WebUploadMode,
};
