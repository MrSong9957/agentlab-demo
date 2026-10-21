-- 013 M1：逐字拷贝自 openai/codex @ ce0323f2f2d3b25e82806cd555237da3db4e1897
-- codex-rs/state/memory_migrations/0002_consolidation_progress.sql（Apache-2.0）。除本来源头注释外未做任何改动。
-- A successful consolidation is the readiness boundary for the version experiment.
CREATE TABLE consolidation_progress (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    max_thread_count INTEGER NOT NULL DEFAULT 0
);
INSERT INTO consolidation_progress (singleton) VALUES (1);
