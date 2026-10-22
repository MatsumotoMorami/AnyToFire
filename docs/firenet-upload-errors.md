# FireNet 单局上传的 400 与 404

2026-10-22 对已保存的 IIDX 34 单局进行核查：成功上传的请求返回 `201 INSERTED`；一份先前被拒绝的原始单局重试后，服务端返回 `400 VALIDATION_FAILED`，violation 为 `request:CONFLICTING_PLAY_LOG`。

该局属于中途死亡：原始 `IIDX34music.attributes.mnum` 与 `best_result.attributes.now_misscount` 均为 `-1`，而 `music_play_log.attributes.miss_num` 为游戏提供的实际数值，`is_sudden_death=true`。本地检查的十份 400 单局均有相同结构；六份 `cflg=1` 且成功上传的单局，三个失误数字段相同且 `is_sudden_death=false`。客户端保持原始 `music.reg` 不变，因此需要 FireNet 服务端在中途死亡时把 `-1` 作为“顶层未知值”处理，不能要求它与日志中的实际失误数相等。

接口文档将 `404 ACCESS_CODE_INVALID` 定义为第三方访问码无效、撤销或轮换。旧版客户端只记录 `HTTP 404`，无法从旧日志断定截图中的 404 是否就是该代码。新版客户端会保留服务端的 `code`、violation 字段和 requestId，避免记录访问码或原始请求内容。路由当前可达：不含访问码的空请求返回 `400 VALIDATION_FAILED`，字段为 `accessCode:REQUIRED`。
