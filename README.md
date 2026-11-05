# Token Quota

一个本地命令行工具，集中查看 Claude Code、Codex 和 Qoder CN 的剩余额度，并支持领取 Qoder CN 每日活动额度。

## 要做什么

- **统一查用量**：展示各平台的已用量、剩余额度、使用比例和重置或到期时间（以平台实际提供的数据为准）。
- **Qoder CN 每日领取**：查询当前活动与领取状态，支持手动领取，后续可接入本机定时任务自动领取；已领取时跳过。
- **方便集成**：提供易读的终端输出和 JSON 输出，供状态栏或其他程序使用。
- **明确数据状态**：显示更新时间；认证过期、请求失败或字段缺失时明确提示，不把未知额度当成零。

## 初步接入方式

- **Claude Code**：读取本机登录凭证，查询用量接口。
- **Codex**：优先查询实时用量，必要时展示本机会话记录中的最近快照，并标注数据时间。
- **Qoder CN**：优先使用官方 SDK 查询额度；参考社区的 campaigns 接口实现每日领取。Linux 下的认证和设备信息读取仍需验证。

凭证只在本机读取，仅向对应平台发送认证请求，不写入仓库或日志。首版以清晰、简单的实现为主，增加短时缓存，避免频繁请求。

## 首版范围

已实现三个平台的用量查询、Qoder CN 手动领取、JSON 输出，以及多设备上报与汇总查看。定时领取、状态栏和提醒后续再做。

## 使用

```sh
tq                      # 查看服务器上各设备汇总的最新额度（未配置服务器时显示本机）
tq --json               # 同上，JSON 输出
tq local                # 只查询本机（60 秒缓存，--refresh 跳过缓存；别名 tq show）
tq local --json --only codex  # JSON 输出，可只查一个平台
tq push                 # 查询本机并上报到服务器
tq qoder status         # Qoder CN 每日活动的领取状态
tq qoder claim          # 领取；已领取时跳过（--dry-run 只检查）
tq setup <服务器地址>     # 新设备配置服务器，令牌从标准输入读取
```

配置文件为 `~/.config/token-quota/config.toml`，字段说明见 `src/config.rs`。Qoder CN 需要个人访问令牌（[创建入口](https://qoder.cn/account/integrations)）。

### 多设备

- 服务器 `tq-server` 只保存额度数字，每个"平台 + 账号"保留最新一条；部署文件在 `deploy/`。
- 只查看的设备（例如 macOS）不需要安装 `tq`：把 `deploy/tq.sh` 里的 `tq` 函数加到 `~/.zshrc` 或 `~/.bashrc`，把服务器令牌放到 `~/.config/token-quota/server_token`（`chmod 600`），然后运行 `tq`。它用 `curl` 读取服务器排好版的文字，时区自动取 `date +%z`。
- 也要上报的设备：取得 `tq` 二进制后运行 `tq setup https://tq.molec.cc`，然后直接运行 `tq`。
- 定时上报：把 `deploy/systemd/` 下的两个文件放到 `~/.config/systemd/user/`，运行 `systemctl --user enable --now tq-push.timer`。
- 同一平台在不同设备登录不同账号时，在各设备的配置 `[accounts]` 里给账号起不同别名，否则会互相覆盖。

### 构建

```sh
cargo build --release                       # 本机
CC_x86_64_unknown_linux_musl=gcc cargo build --release --target x86_64-unknown-linux-musl  # 静态 Linux 二进制
```

## 参考

- [Qoder CN 官方用量 SDK](https://docs.qoder.cn/cli/sdk/cost-usage)
- [每日领取 100 Credits 活动规则](https://docs.qoder.cn/events/100credits)
- [社区独立领取脚本 qoder_signin.py](https://github.com/88lin/workbuddy-auto-signin/blob/main/qoder_signin.py)

部分查询和领取接口未公开承诺兼容性，实际可用性需在接入时验证。
