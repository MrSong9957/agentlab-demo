//! M9 tests share real stores, a stepped seconds clock and isolated directories.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use terminal_chat::memory::{
    Clock, MemoryStore, Phase2JobClaimOutcome, SelectedOutput, Stage1JobClaimOutcome,
};

pub const T0: i64 = 1_700_000_000;
pub const DAY: i64 = 86_400;

pub struct Env {
    pub dir: PathBuf,
    pub root: PathBuf,
    pub sessions: PathBuf,
    pub time: Arc<AtomicI64>,
    pub store: MemoryStore,
}

impl Env {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tchat-m9-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let root = dir.join("memories");
        let sessions = dir.join("sessions");
        for p in [&sessions, &dir.join("home"), &dir.join("config")] {
            std::fs::create_dir(p).unwrap();
        }
        let time = Arc::new(AtomicI64::new(T0));
        let clock_time = Arc::clone(&time);
        let store =
            MemoryStore::open(&root, Arc::new(move || clock_time.load(Ordering::SeqCst))).unwrap();
        Self {
            dir,
            root,
            sessions,
            time,
            store,
        }
    }
    pub fn clock(&self) -> Clock {
        let time = Arc::clone(&self.time);
        Arc::new(move || time.load(Ordering::SeqCst))
    }
    pub fn set_time(&self, t: i64) {
        self.time.store(t, Ordering::SeqCst);
    }
    pub fn now(&self) -> i64 {
        self.time.load(Ordering::SeqCst)
    }
    pub fn seed(&self, id: &str, source: i64) {
        self.store.register_thread(id, "enabled", source * 1000).unwrap();
        let Stage1JobClaimOutcome::Claimed { ownership_token } = self
            .store
            .try_claim_stage1_job(id, "m9-seed", source, 3600, 8)
            .unwrap()
        else {
            panic!("claim {id}")
        };
        assert!(self
            .store
            .mark_stage1_job_succeeded(
                id,
                &ownership_token,
                source,
                &format!("raw-{id}"),
                &format!("summary-{id}"),
                None
            )
            .unwrap());
    }
    pub fn select(&self, ids: &[&str]) {
        let Phase2JobClaimOutcome::Claimed {
            ownership_token,
            input_watermark,
        } = self
            .store
            .try_claim_global_phase2_job("m9-select", 3600)
            .unwrap()
        else {
            panic!("global claim")
        };
        let selected: Vec<_> = ids
            .iter()
            .map(|id| SelectedOutput {
                thread_id: id.to_string(),
                source_updated_at: self
                    .store
                    .stage1_output(id)
                    .unwrap()
                    .unwrap()
                    .source_updated_at,
            })
            .collect();
        assert!(self
            .store
            .mark_global_phase2_job_succeeded(&ownership_token, input_watermark, &selected)
            .unwrap());
    }
    pub fn ids(&self) -> Vec<String> {
        self.store
            .list_stage1_outputs()
            .unwrap()
            .into_iter()
            .map(|r| r.thread_id)
            .collect()
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn answer(content: &str) -> serde_json::Value {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":content}}]})
}
pub fn tools(calls: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let calls: Vec<_> = calls.iter().enumerate().map(|(i,(name,args))| serde_json::json!({"id":format!("call-{i}"),"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect();
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":calls}}]})
}
pub fn artifacts(manual: &str, summary: &str) -> serde_json::Value {
    tools(&[
        (
            "mem_write",
            serde_json::json!({"path":"MEMORY.md","content":manual}),
        ),
        (
            "mem_write",
            serde_json::json!({"path":"memory_summary.md","content":summary}),
        ),
    ])
}
pub fn git(root: &Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .arg(format!("--git-dir={}", root.join(".git").display()))
        .arg(format!("--work-tree={}", root.display()))
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

pub struct Step {
    pub required: Vec<String>,
    pub tools_only: bool,
    pub response: serde_json::Value,
}
impl Step {
    pub fn request(required: &[&str], response: serde_json::Value) -> Self {
        Self {
            required: required.iter().map(|s| s.to_string()).collect(),
            tools_only: false,
            response,
        }
    }
    pub fn tool_result(required: &[&str], response: serde_json::Value) -> Self {
        Self {
            tools_only: true,
            ..Self::request(required, response)
        }
    }
}
pub struct Script {
    steps: std::cell::RefCell<std::collections::VecDeque<Step>>,
    pub requests: std::cell::RefCell<Vec<serde_json::Value>>,
}
impl Script {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: std::cell::RefCell::new(steps.into()),
            requests: Default::default(),
        }
    }
    pub fn send(&self, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let step = self
            .steps
            .borrow_mut()
            .pop_front()
            .expect("unexpected model call / broken upstream stage");
        let observed = if step.tools_only {
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "tool")
                .map(|m| m["content"].as_str().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            body.to_string()
        };
        for expected in &step.required {
            assert!(observed.contains(expected),"required upstream evidence {expected:?} missing from actual request/tool result: {observed}");
        }
        self.requests.borrow_mut().push(body.clone());
        Ok(step.response)
    }
    pub fn finished(&self) {
        assert!(
            self.steps.borrow().is_empty(),
            "expected model stages were not executed"
        );
    }
}
