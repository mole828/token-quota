//! 服务器端的快照存储：每个"平台 + 账号"只保留最新的一条，持久化为一个 JSON 文件。

use crate::config::write_atomic;
use crate::snapshot::{SCHEMA_VERSION, Snapshot, Source};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Store {
    latest: BTreeMap<String, Snapshot>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    Stored,
    /// 已有同样新或更新的数据。
    Outdated,
    /// 快照格式版本不认识。
    UnsupportedSchema,
}

impl Store {
    fn key(snapshot: &Snapshot) -> String {
        format!("{}/{}", snapshot.platform, snapshot.account)
    }

    /// 时间更新的快照替换旧快照；时间相同时，实时数据优先于会话记录。
    pub fn merge(&mut self, snapshot: Snapshot) -> MergeOutcome {
        if snapshot.schema != SCHEMA_VERSION {
            return MergeOutcome::UnsupportedSchema;
        }
        let key = Self::key(&snapshot);
        if let Some(existing) = self.latest.get(&key)
            && !is_newer(&snapshot, existing)
        {
            return MergeOutcome::Outdated;
        }
        self.latest.insert(key, snapshot);
        MergeOutcome::Stored
    }

    pub fn snapshots(&self) -> Vec<Snapshot> {
        self.latest.values().cloned().collect()
    }

    pub fn load(path: &Path) -> Result<Store> {
        if !path.exists() {
            return Ok(Store::default());
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("读取状态文件失败：{}", path.display()))?;
        let snapshots: Vec<Snapshot> =
            serde_json::from_str(&text).with_context(|| format!("状态文件格式错误：{}", path.display()))?;
        let mut store = Store::default();
        for snapshot in snapshots {
            store.merge(snapshot);
        }
        Ok(store)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.snapshots())?;
        write_atomic(path, &json)
    }
}

fn is_newer(candidate: &Snapshot, existing: &Snapshot) -> bool {
    if candidate.fetched_at != existing.fetched_at {
        return candidate.fetched_at > existing.fetched_at;
    }
    candidate.source == Source::Live && existing.source != Source::Live
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Platform;
    use chrono::{DateTime, Utc};

    fn snap(platform: Platform, account: &str, device: &str, source: Source, at: i64) -> Snapshot {
        let at: DateTime<Utc> = DateTime::from_timestamp(at, 0).unwrap();
        Snapshot::new(platform, account, device, source, at)
    }

    #[test]
    fn newer_snapshot_replaces_older_from_any_device() {
        let mut store = Store::default();
        assert_eq!(store.merge(snap(Platform::Codex, "a", "laptop", Source::Live, 100)), MergeOutcome::Stored);
        assert_eq!(store.merge(snap(Platform::Codex, "a", "desktop", Source::Live, 200)), MergeOutcome::Stored);
        assert_eq!(store.snapshots()[0].device, "desktop");
    }

    #[test]
    fn older_snapshot_is_ignored() {
        let mut store = Store::default();
        store.merge(snap(Platform::Codex, "a", "laptop", Source::Live, 200));
        assert_eq!(store.merge(snap(Platform::Codex, "a", "desktop", Source::SessionLog, 100)), MergeOutcome::Outdated);
        assert_eq!(store.snapshots()[0].device, "laptop");
    }

    #[test]
    fn live_wins_tie_against_session_log() {
        let mut store = Store::default();
        store.merge(snap(Platform::Codex, "a", "laptop", Source::SessionLog, 100));
        assert_eq!(store.merge(snap(Platform::Codex, "a", "desktop", Source::Live, 100)), MergeOutcome::Stored);
        assert_eq!(store.merge(snap(Platform::Codex, "a", "laptop", Source::SessionLog, 100)), MergeOutcome::Outdated);
    }

    #[test]
    fn accounts_and_platforms_are_kept_separately() {
        let mut store = Store::default();
        store.merge(snap(Platform::Codex, "a", "d", Source::Live, 100));
        store.merge(snap(Platform::Codex, "b", "d", Source::Live, 100));
        store.merge(snap(Platform::Claude, "a", "d", Source::Live, 100));
        assert_eq!(store.snapshots().len(), 3);
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let mut store = Store::default();
        let mut s = snap(Platform::Qoder, "a", "d", Source::Live, 100);
        s.schema = SCHEMA_VERSION + 1;
        assert_eq!(store.merge(s), MergeOutcome::UnsupportedSchema);
        assert!(store.snapshots().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut store = Store::default();
        store.merge(snap(Platform::Claude, "a", "d", Source::Live, 100));
        store.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap().snapshots(), store.snapshots());
    }
}
