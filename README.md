# AnyToFire

独立的 IIDX 34 成绩导出插件。`AnyToFire.dll` 注入游戏后，在玩家登录并取得个人成绩缓存时自动保存全曲最佳成绩；退出游戏后用 `AnyToFire.exe` 查看、重命名、导出或合并。不会向任何服务器上传数据。

## 使用

1. 将 `dist/AnyToFire.dll` 与 `dist/AnyToFire.exe` 复制到 IIDX 根目录（与 `bm2dx.dll` 同目录）。
2. 在 Spice2x 的 DLL 注入列表中加入 `AnyToFire.dll`。若同时使用 `IIDXScore.dll`，请将 AnyToFire 排在 IIDXScore 后面。该链式模式导出的成绩也包含 IIDXScore 对个人成绩缓存的本地覆盖。
3. 启动游戏并登录玩家，等待游戏进入曲选。DLL 会在 `AnyToFire_exports/<时间-进程号>/` 写入 `manifest.json`、`fervidex.json`、`konami_sp.csv`、`konami_dp.csv`。每次读到 SP/DP 成绩会更新同一份导出。
4. 退出游戏后运行 `AnyToFire.exe`。列表支持多选；顶部按钮可另存 Fervidex JSON、KONAMI CSV，或合并选中成绩。合并保留所有子成绩，并新增一份合并成绩。下方输入新名称后点击“重命名选中成绩”。

DLL 日志在游戏根目录的 `AnyToFire.log`。EXE 默认读取同目录的 `AnyToFire_exports`，也可用 `AnyToFire.exe --data <导出目录>` 指定位置。项目使用 Rust；在本机运行 `build.ps1` 会重新编译并复制 DLL/EXE 到 `dist`。

## 数据范围和格式

- 游戏的 `pc.get` 个人资料包含公开 IIDX ID（`xxxx-xxxx`）、昵称以及服务器内部 `pid`。插件用 `pid` 与 `music.getrank` 的玩家键精确关联，不按登录顺序猜测 1P/2P。
- `music.getrank` 全量个人成绩缓存提供各谱面的最佳 EX、灯和 BP。自动导出的 `fervidex.json` 使用 Fervidex static-profile 的 `scores/<song_id>/<chart>/{clear_type,ex_score,miss_count}` 结构。[格式参考](https://github.com/zkrising/Tachi/blob/main/typescript/server/src/lib/score-import/import-types/ir/fervidex-static/parser.ts)。
- KONAMI CSV 使用相同的列名、顺序和 UTF-8 BOM，SP 与 DP 分文件。登录时的全量缓存没有每首最佳成绩对应的 PGreat、Great、游玩次数和最后游玩时间；这些列留空，不伪造数值。因而 CSV 适合查看和保留成绩，不保证所有严格校验的第三方导入器接受。
- 合并按曲目 ID 与谱面分别取最高 EX、最高灯和最低已知 BP。曲库哈希不同或已知 IIDX ID 冲突时拒绝合并，避免误合并不同谱面或玩家。

当前 DLL 只识别本机已核对的 IIDX 34 `2026091600` 版 `bm2dx.dll`。ABI 校验不通过时只写日志，不安装钩子。尚未经过一次实际登录验证；首次使用后请检查 `AnyToFire.log` 中的 `pc.get` 与导出行数，再核对生成文件中的 IIDX ID、昵称和成绩数量。

## 新界面演示

双击 `ui-demo/index.html` 可在浏览器中查看新的成绩库界面提案。页面提供搜索、筛选、多选合并、重命名及导出流程的交互预览。所列玩家和成绩均为演示数据；此页面尚未连接本地导出目录，也不会修改现有 EXE。
演示界面的火焰图标取自 [FireNet 官网](https://firenet.iidx.pro/)的站点图标。
