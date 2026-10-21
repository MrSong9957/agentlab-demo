-- 013 M1 本地适配（G6，登记偏离）：上游 threads 登记位于 state 库主迁移
-- （含来源/归档/分区等大量列，见手册 §4.1）；本项目会话文件不落 state 库，
-- 仅以最小列随记忆库落建——memory_mode 默认 'enabled'（上游候选筛选要求值），
-- updated_at_ms 为候选扫描（M2）使用的会话更新时间。非上游逐字拷贝。
CREATE TABLE threads (
    thread_id TEXT PRIMARY KEY,
    memory_mode TEXT NOT NULL DEFAULT 'enabled',
    updated_at_ms INTEGER NOT NULL
);
