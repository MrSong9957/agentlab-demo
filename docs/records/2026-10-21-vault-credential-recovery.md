# /vault Git 凭据恢复与推送

日期：2026-10-21。背景：容器重建后 GCM、GPG、pass、Git credential 配置丢失，`git push` 报 `could not read Username for 'https://github.com'`。

## 提交与推送结果

- 本地提交：`82012cc docs(wiki): 补充文档变更验证范围并明确 agents 目录归属`（2 文件，+3/-1），连同此前未推送的 `978b204` 一起推送。
- 推送：`27737b4..82012cc  master -> master`，退出码 0。
- 远端 `refs/heads/master`：`82012cc448e13ad74533ea25002a30218c516405`。
- 推送后 `git status -sb`：`## master...origin/master`，工作区干净。

## 凭据恢复

1. 安装 `gnupg 2.4.4`、`pass 1.7.4`、`pinentry-curses`、`libicu74`；安装 GCM 2.6.1 到 `/usr/local/lib/gcm`，wrapper `/usr/local/bin/git-credential-manager` 补 `GPG_TTY`（非 TTY 回退 `/dev/null`）。
2. 生成无口令 GPG 密钥并执行 `pass init`，沿用 2026-10-05 记录的无人值守取舍；`~/.password-store` 700，凭据文件 600。
3. Git 配置：`credential.helper=manager`、`credential.credentialStore=gpg`、`credential.interactive=auto`、GitHub provider=`github`；`remote.origin.url` 只含用户名 `MrSong9957`，不含 token。
4. 旧恢复路径核对：ZCode 历史记录中唯一可提取的 GitHub token 是已知的 40 位失效 token，GitHub API 返回 401；没有可用的旧 token。
5. 改用 OAuth 设备流：`git-credential-manager github login --device --username MrSong9957`，账号 `MrSong9957` 授权成功，凭据写入 GPG 加密存储。设备码临时日志已删除。

## 验证

- `git credential fill` 返回凭据：present。
- `git push --dry-run origin master`：退出码 0，`27737b4..82012cc`。
- 正式 `git push origin master`：退出码 0；`git ls-remote origin refs/heads/master` 与本地 HEAD 一致。
- `scripts/git-token-check.sh /vault`：退出码 0，remote/config 无凭据嵌入，连通性正常。
- `git-credential-manager github list`：`MrSong9957`。

## 安全提示

- 恢复扫描发现一个历史 ZCode artifact 同时含上述失效 GitHub token 和一个 `DEEPSEEK_API_KEY`；该模型密钥与 `/workspace/.env` 当前值的哈希一致，扫描输出中曾显示该值。未将其写入凭据存储或仓库，但建议轮换该模型密钥。
- GCM、GPG 私钥和 password-store 属容器层，重建会再次丢失。后续恢复优先使用 OAuth 设备流或新 PAT；不要再依赖 ZCode 日志中的旧 token。
