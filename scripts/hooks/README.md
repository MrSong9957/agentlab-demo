# 提交前密钥扫描（026）

## 安装 gitleaks（一次性，任意用户目录）

```sh
cd /tmp
curl -fsSL -O https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz
curl -fsSL -O https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_checksums.txt
grep 'linux_x64.tar.gz$' gitleaks_8.30.1_checksums.txt | sha256sum -c -   # 必须 OK
tar xzf gitleaks_8.30.1_linux_x64.tar.gz gitleaks
mkdir -p ~/.local/bin && mv gitleaks ~/.local/bin/    # 确保 ~/.local/bin 在 PATH
```

校验失败则停止安装并排查来源；不得跳过校验。

## 安装 pre-commit 钩子

```sh
cd <仓库根>
if [ -n "$(git config core.hooksPath)" ]; then
  echo "core.hooksPath 已设为 $(git config core.hooksPath)：请把 scripts/hooks/pre-commit 手动放入该目录（不要覆盖已有同名钩子）"
else
  [ -e .git/hooks/pre-commit ] && echo "已存在 .git/hooks/pre-commit，请手动合并" || cp scripts/hooks/pre-commit .git/hooks/pre-commit
fi
```

## 行为

- 扫描对象：暂存区实际内容（`gitleaks protect --staged`），不读取未跟踪文件。
- 失败语义：gitleaks 缺失／执行失败／发现命中 → 拒绝提交（fail closed）。
- 输出：`--redact` 脱敏，只含文件与规则名。
- 豁免：仅 `.gitleaks.toml` 中值锚定的三处已核查测试样例；其他疑似密钥一律拦截。
- 绕过边界：`git commit --no-verify` 可绕过本地钩子（本地钩子的固有边界）；
  闭环需远端/CI 扫描，属后续独立决策。
