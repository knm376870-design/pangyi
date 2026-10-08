# 阶段 2：Windows TSF 输入法 —— 现状与验证

## 已完成

| 环节 | 状态 | 验证方式 |
|---|---|---|
| COM 注册 | ✅ | `probe` 里 `CoCreateInstance(我们的 CLSID)` 成功 |
| TSF 注册 | ✅ | `EnumLanguageProfiles` 列出我们、Enable=1、8 个类别 |
| 能被激活 | ✅ | `ActivateLanguageProfile` 成功 |
| DLL 加载 | ✅ | 日志 `[pid N] DllGetClassObject 被调用` |
| 服务激活 | ✅ | 日志 `Activate` + `TfClientId=49` |
| **按键接收器注册** | ✅ | 日志 `AdviseKeyEventSink ok (TfClientId=49)` |
| 焦点事件 | ✅ | 日志 `ThreadMgr::OnSetFocus` |
| 编辑会话 | 已实现，未验证 | `ITfEditSession` + `RequestEditSession` |

## 关键修复（按发现顺序）

1. **`Category\Item` 反向索引树**——只做了一半的类别注册，补上后 TSF 才加载我们
2. **日志权限**——`C:\ProgramData\Pangyi\ime.log` 只有管理员可写，普通进程的日志全丢了，
   这是早期反复误判的根源。已授予 `Users` 修改权限
3. **`AdviseKeyEventSink` 必须用 `TfClientId`**——之前传的是线程 ID，
   导致一直返回 `0x80040201`，按键接收器从未注册成功

## 未验证

**真实应用里的按键 → 上屏。** 原因：自动化宿主缺少 TSF 应用侧的完整管道
（`ITfDocumentMgr` / `ITfContext` / 消息过滤），
**对照实验证明这一点**：用同一个宿主激活青简（确定可用的输入法），
`TestKeyDown` 同样返回 `eaten=false`，也打不出字。
→ 宿主不可用于判定输入法本身。

## 怎么手工验证（30 秒）

1. 打开**全新**的记事本（或任何 TSF 应用）
2. Win+Space 切到「旁译」——**敲字母时不弹候选框**的那个
3. 敲 `kaifa` + 空格
4. 看 `C:\ProgramData\Pangyi\ime.log`：
   ```
   [pid xxxx] OnTestKeyDown wparam=75 我要=true
   [pid xxxx] 上屏 key=kaifa 候选=N
   [pid xxxx] DoEditSession 上屏成功 2 字
   ```

## 诊断工具（都在 `ime/src/bin/`）

- `probe` —— 问 TSF：注册了吗？能激活吗？能创建对象吗？
- `host` —— 自建 TSF 宿主（**已知不足**，仅用于触发激活，不能验证按键）
- `install_tip` —— 调 `InstallLayoutOrTip` 给当前用户装输入法

## 已知限制

- 只有 64 位（32 位程序如微信/WPS 用不了）
- 没有候选窗口（按空格直接取第一个候选）
- 没有拼音下划线（按下字母不进文档，屏幕上看不到正在打什么）
