# Token Quota 查看函数：不需要安装 tq，只要 curl 和 date。sh / bash / zsh 通用。
#
# 用法：把本文件内容加到 ~/.zshrc 或 ~/.bashrc，然后运行 tq。
# 令牌放在 ~/.config/token-quota/server_token（chmod 600）。
# 可用环境变量 TQ_SERVER_URL 换服务器，TQ_TZ 换时区（默认取本机，格式 +0800）。
tq() {
  curl -sS --fail-with-body \
    -H "Authorization: Bearer $(cat "$HOME/.config/token-quota/server_token")" \
    "${TQ_SERVER_URL:-https://tq.molec.cc}/v1/text?tz=${TQ_TZ:-$(date +%z)}"
}
