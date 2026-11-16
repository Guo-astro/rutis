// Re-check of #634's 1368 protocol rules against the current 0.22 sources.
//
// `formal-trace.mjs --write` uses this module to generate
// docs/formal-trace/recheck-0.22.md and to take the tracked status of a
// protocol row from the re-check instead of the issue snapshot. `--check`
// verifies the generated file, the anchors it names and the per-partition
// totals.
//
// The data here is the part a script cannot read off the tree: which document
// is the current source of a rule family in 0.22, which old rule a current
// spec rule replaces, and which rule text relies on a mechanism that 0.19 or
// 0.22 removed. Every entry carries the file that settles it; `--check` reads
// those files (the anchor ids, the removed names) so a stale entry fails.
//
// Ownership, so the two scripts cannot drift apart: this module owns the
// per-family current sources (`SOURCES`), the old-section anchors (`ANCHORS`),
// the withdrawn rules (`SUPERSEDED`), the no-source reasons (`UNMAPPED`), the
// renamed-mechanism notes (`NOTES`) and the guarantee families
// (`REGISTRY_FAMILIES`). `formal-trace-semantic.mjs` owns the two partitions
// re-extracted semantically (`SEMANTIC`, `SEMANTIC_REGISTRY`, `SEMANTIC_DOCS`,
// `SEMANTIC_PAGE`). `formal-trace.mjs`
// owns only the presentation vocabulary (columns, status words, the file list)
// and reads the rest from these two modules; it derives nothing in a second
// copy. `--check` verifies the literals that must agree: the family list here,
// the register's own families, the families the generated files cover, and the
// `SOURCES` keys against the id prefixes.

// The current source of a rule family. `kind` is what the re-check may claim:
// `spec` a current specification, `design` a current design, `guide` a current
// user document, `historical` a design a later design or the code replaced.
export const SOURCES = {
  EVP: {
    doc: 'docs/design-events-protocol-2026-10-18.md',
    label: '事件协议规格',
    kind: 'spec',
    note: '0.18 基线；§2.1 的 events-v2 与 §九 的 export / import 已被 0.19 取代（本表逐条标注）',
  },
  EVD: {
    doc: 'docs/design-events-2026-10-18.md',
    label: '统一事件设计',
    kind: 'design',
    note: '现行设计',
  },
  SES: {
    doc: 'docs/protocol-session.md',
    label: '会话协议规格',
    kind: 'spec',
    note: '会话层唯一的现行规格；值标记的线上格式仍见 `design-protocol-plugins-2026-09-25.md` §6.1',
  },
  VAL: {
    doc: 'docs/design-value-types-2026-10-15.md',
    label: '跨语言值类型',
    kind: 'design',
    note: '定稿，0.10 起实现；会话内编码见会话规格 §2.2 与协议插件设计 §6.1',
  },
  TAG: {
    doc: 'docs/design-session-tags-2026-10-12.md',
    label: '会话标签设计',
    kind: 'design',
    note: '已实现；协议 4 的调用链规则见会话规格 §5.2',
  },
  M1: {
    doc: 'docs/design-multilang-m1-2026-10-04.md',
    label: '多语言 M1',
    kind: 'design',
    note: '已实现；§7 的跨会话转发见会话规格 §6',
  },
  CHN: {
    doc: 'docs/design-protocol-channel-decoupling-2026-10-03.md',
    label: '网络栈设计',
    kind: 'design',
    note: '连接层当前来源；0.19 的 `rutis-host/dial` / `listen` 删除了注册代次、接入点与 `ChannelInfo`（`design-remote-session-2026-10-28.md` §三、§八）',
  },
  SYN: {
    doc: 'docs/design-multilang-runtimes-2026-10-03.md',
    label: '多语言运行时设计',
    kind: 'historical',
    note: '0.9 基线的设计稿（未实现）；实现与 §九 的等待图约定见 `design-multilang-m1`、`design-multilang-m2` 与登记表 `bridge/sync-wait-cycle-refused`',
  },
  M2: {
    doc: 'docs/design-multilang-m2-2026-10-04.md',
    label: '多语言 M2',
    kind: 'design',
    note: '已实现',
  },
  MNT: {
    doc: 'docs/design-protocol-plugin-mount.md',
    label: '跨进程挂载设计',
    kind: 'design',
    note: '现行设计；`build.rs` 清单已在 0.17 退休（`design-rust-service-exposure-2026-10-21.md` R10）',
  },
  REQ: {
    doc: 'docs/requirements-protocol-plugins.md',
    label: '协议插件需求',
    kind: 'design',
    note: '现行需求',
  },
  RTP: {
    doc: 'docs/design-cordis-runtime-plugin-2026-10-03.md',
    label: 'Cordis 运行时插件设计',
    kind: 'design',
    note: '已实施',
  },
  INS: {
    doc: 'docs/design-instance-services-2026-10-08.md',
    label: '实例内的服务名',
    kind: 'design',
    note: '0.16 的设计提案；行服务与声明集合后来由 `design-declared-services-2026-10-29.md` 补充',
  },
  MSV: {
    doc: 'docs/design-rust-service-exposure-2026-10-21.md',
    label: 'Rust 服务自动对外',
    kind: 'design',
    note: '已实现（0.15）',
  },
  REM: {
    doc: 'docs/design-remote-plugins-2026-10-03.md',
    label: '远程插件设计',
    kind: 'historical',
    note: '0.19 由 `design-remote-session-2026-10-28.md` 取代连接、身份、注册与节点名单部分（§八、§十二）',
  },
  DIA: {
    doc: 'docs/design-remote-runtime-dial-2026-10-14.md',
    label: '远程运行时拨出设计',
    kind: 'design',
    note: '定稿；0.19 删除了接入点、吊销与 `runtimes.remote`，拨出改为 `rutis-host/dial` 的行',
  },
  APP: {
    doc: 'docs/design-app-integration-2026-10-14.md',
    label: '应用接入设计',
    kind: 'design',
    note: '定稿，未实现；0.19 改写了 D2、D6（每个应用自己装插件）',
  },
  NOD: {
    doc: 'docs/guide/nodes.md',
    label: '连接节点指南',
    kind: 'guide',
    note: '现行用户指南；0.19 起节点也是一条 `rutis-host/dial` / `listen` 的行，事件是一条总线，没有 export / import',
  },
}

// The rule families whose rows are guarantee-register rows: their current
// source is the register itself (one row per id, a–g suffixes split from the
// base id).
export const REGISTRY_FAMILIES = ['core', 'events', 'boundary', 'instance', 'bridge', 'host', 'dsh', 'sdk', 'loader', 'swift']

// A current anchor of a session-spec rule, by the old source section. Only the
// sections the current spec actually covers get an id; the rest are named as
// outside its scope and stay in the gap list. `--check` verifies every id
// below is in `docs/protocol-session.md`.
export const ANCHORS = [
  [/^SES §一/, ['SESS-46', 'SESS-52', 'SESS-54'], '引用、对象与中继'],
  [/^SES §二/, ['SESS-52', 'SESS-54'], '对象代理与中继'],
  [/^SES §四/, [], '三层架构与生命周期权威；会话规格不覆盖'],
  [/^SES §五/, [], 'runtime 组与部署；会话规格不覆盖'],
  [/^SES §(?:六|6\.1)/, ['SESS-9', 'SESS-10', 'SESS-11'], '值标记；格式仍见协议插件设计 §6.1'],
  [/^SES §7\.1/, ['SESS-46'], '引用身份'],
  [/^SES §7\.2/, ['SESS-48'], '引用失效与错误'],
  [/^SES §7\.3/, ['SESS-52'], '对象属性与方法调用'],
  [/^SES §8/, ['SESS-47', 'SESS-51'], '授予、释放与会话结束'],
  [/^SES §九/, [], '服务图、装载与恢复；见多语言 M1 与挂载设计'],
  [/^SES §十一/, [], '事件；现行来源是事件协议规格'],
  [/^SES §十二/, ['SESS-2', 'SESS-63', 'SESS-70', 'SESS-74'], '传输、畸形帧与上限'],
  [/^SES §十三/, [], '权限与诊断；会话规格把它列为明确不保证或连接层'],
  [/^SES §十四/, ['SESS-79', 'SESS-80', 'SESS-81'], '迁移与版本'],
  [/^SES §十(?!一|二|三|四)/, ['SESS-18', 'SESS-38', 'SESS-39', 'SESS-45'], '调用顺序、同步等待与取消'],
  [/^VAL /, ['SESS-9', 'SESS-10', 'SESS-11'], '值标记；逐语言映射见值类型设计'],
  [/^TAG /, ['SESS-8', 'SESS-36', 'SESS-37'], '调用链、标签与 `rebase`'],
  [/^M1 §7\./, ['SESS-36', 'SESS-54'], '跨会话转发与中继'],
  [/^M1 /, [], '行、服务与运行时；会话规格不覆盖'],
  [/^CHN 范围|^CHN §分层/, ['SESS-1', 'SESS-2'], '会话跑在一条通道上'],
  [/^CHN §Channel 契约/, ['SESS-1', 'SESS-18', 'SESS-75'], '有序交付、帧序与背压'],
  [/^CHN §分帧/, ['SESS-2', 'SESS-70'], '一帧一条消息与上限'],
  [/^CHN §畸形帧/, ['SESS-63', 'SESS-71', 'SESS-72'], '畸形帧与失败范围'],
  [/^CHN §逻辑通道与物理连接/, ['SESS-1'], '一个会话一条逻辑通道'],
  [/^CHN §本机子进程端点/, ['SESS-1'], '子进程承载的通道'],
  [/^CHN §WebSocket 绑定/, ['SESS-70', 'SESS-72'], 'WebSocket 的大小上限与关闭'],
  [/^CHN §阶段与版本/, ['SESS-3', 'SESS-80'], '协议版本与能力'],
  [/^CHN /, [], '连接层；0.19 改为连接插件的行'],
]

// Rules whose subject 0.19 or 0.22 removed, with the file that settles it.
// The re-check status is `已撤回`; the row stays, the replacement is the gap.
export const SUPERSEDED = [
  {
    ids: ['EVP-2.1-1', 'EVP-2.1-2', 'EVP-2.2-1', 'EVP-2.2-2', 'EVP-2.2-3', 'EVP-2.2-4'],
    why: '协议 4 没有 `events-v2` 能力（`protocol-session.md` `SESS-16`），0.18 及更早的对端在握手时按版本被拒（`SESS-23`、`SESS-79`）',
    by: 'docs/protocol-session.md',
  },
  {
    ids: ['EVP-2.4-3', 'EVP-3-4', 'EVP-9-1', 'EVP-9-2', 'EVP-9-9', 'EVP-9-10', 'EVP-9-12', 'EVP-9-20', 'NOD-3-1'],
    why: '0.19 删除节点的 `export` / `import` 名单，节点之间是一条总线、每个名字都在（`design-remote-session-2026-10-28.md` §八、§十二；登记表 `events/node-links` 重写）',
    by: 'docs/design-remote-session-2026-10-28.md',
  },
  {
    ids: ['events/v2-required a', 'events/v2-required b', 'events/node-links a', 'events/node-links b', 'events/link-export-import a', 'events/link-export-import b', 'events/flat b', 'loader/peer-rows a', 'events/node-config a', 'events/node-config b', 'events/no-multi-hop'],
    why: '0.19 删除了 `events-v2`、节点 `export` / `import` 名单、`peer:` 行与 `events: {out, in}`；登记表里这些行已删除或改写（`design-remote-session-2026-10-28.md` §八、§十二）',
    by: 'docs/guarantees.md',
  },
  {
    ids: ['bridge/revocation-closes-only-its-connections a', 'bridge/revocation-closes-only-its-connections b', 'bridge/revocation-refuses-reconnects a', 'bridge/revocation-refuses-reconnects b', 'bridge/revocation-refuses-reconnects c', 'host/env-tokens-not-revocable', 'loader/runtime-connection-instances a', 'loader/runtime-connection-instances b', 'loader/runtime-connection-instances c'],
    why: '0.19 删除 `Revocations` / `Revocable`、接入点与每条连接一个实例；断开现有连接靠停掉那一行（`design-remote-session-2026-10-28.md` §八、§十二）',
    by: 'docs/guarantees.md',
  },
  {
    ids: ['REM-3.1-1', 'REM-3.1-5', 'REM-3.1-6', 'REM-3.1-7', 'REM-3.2-7', 'REM-3.3-2', 'REM-5.1-2', 'REM-5.1-8', 'REM-5.1-9', 'REM-5.2-2', 'REM-5.2-4', 'REM-5.2-5', 'REM-5.2-8', 'REM-5.2-13', 'REM-5.2-15', 'REM-5.3-1', 'REM-5.3-2', 'REM-5.3-3', 'REM-5.3-6', 'REM-6.1-4', 'REM-6.2-10', 'REM-6.2-12', 'DIA-2-2', 'DIA-4-9', 'DIA-5-5', 'DIA-5-6', 'DIA-5-7', 'DIA-5-8', 'DIA-5-9', 'DIA-5-10', 'DIA-5-11', 'DIA-5-12', 'DIA-5-13', 'DIA-5-14', 'DIA-5-15', 'DIA-7-1', 'NOD-3-5', 'NOD-3-8', 'NOD-4-1'],
    why: '0.19 删除 `Transport`、`Identity`、`Registration`、接入点、`Revocations`、`PeerRows`、`peer:` 行、`runtimes.remote`、compat 格式与节点 export / import（`design-remote-session-2026-10-28.md` §八、§十二；`migration-0.19.md`）',
    by: 'docs/design-remote-session-2026-10-28.md',
  },
  {
    ids: ['REM-3.2-10', 'REM-3.2-11', 'REM-3.2-12'],
    why: '0.19 删除 `Identity`、`Registration`、`Revocations` 与 link 插件，本仓库没有共享物理连接的多路复用，也没有 Adapter 通知；远程插件设计 §11（#636）的复核结论是「删去」，这三条没有可核对的对象',
    by: 'docs/design-remote-plugins-2026-10-03.md',
  },
  {
    ids: ['SES-6.1-23', 'TAG-2-2', 'SYN-9-21', 'SYN-9-22'],
    why: '协议 4 只有端点格式，没有 compat 格式、`sync-wait` 能力或 `ChannelInfo`（`protocol-session.md` `SESS-12`、`SESS-13`、`SESS-16`）',
    by: 'docs/protocol-session.md',
  },
  {
    ids: ['CHN-1-4', 'CHN-1-6', 'CHN-2-7', 'CHN-5-7', 'CHN-5-8', 'CHN-6-2', 'CHN-8-1', 'CHN-8-5', 'CHN-8-6', 'CHN-8-7', 'CHN-8-9'],
    why: '0.19 删除 `ChannelInfo`、注册代次、接入点与吊销；身份来自握手，路由由 `rutis-host/listen` 的 token 比对（`design-remote-session-2026-10-28.md` §三、§八）',
    by: 'docs/design-remote-session-2026-10-28.md',
  },
  {
    ids: ['CHN-4-11'],
    why: '未知字段现在七种实现都在调用级拒绝（`protocol-session.md` `SESS-66`、`SESS-67`）；“Node 与 Python 忽略”不再成立',
    by: 'docs/protocol-session.md',
  },
  {
    ids: ['MNT-10-4'],
    why: '0.18 起对象引用对所有协议 4 实现都可传（`design-object-refs-2026-10-27.md`；`protocol-session.md` `SESS-52`）；“Rust 侧对象不能以引用交给 Node / Python”不再成立',
    by: 'docs/protocol-session.md',
  },
  {
    ids: ['MNT-11-1'],
    why: '0.17 起 `build.rs` 的 `Exports` 清单退休（`design-rust-service-exposure-2026-10-21.md` R10；`#647`）；标注仍是自动对外的方式',
    by: 'docs/design-rust-service-exposure-2026-10-21.md',
  },
  {
    ids: ['INS-5-5'],
    why: '`peer:` 行在 0.19 删除，改为 `<连接行>:<插件>`（`design-remote-session-2026-10-28.md` §八）',
    by: 'docs/design-remote-session-2026-10-28.md',
  },
  {
    ids: ['MSV-4-2c'],
    why: '0.19 删除 `check()` 谓词门控：`internal::provide_checked`、`refresh`、`service::Check`、`Exposure` 的 `check`、`DependencyStatus::CheckPending` / `CheckRejected` / `CheckPanicked`，写明没有替代；入口与原生绑定一起受它门控没有对象',
    by: 'docs/migration-0.19.md',
  },
]

// The reason a rule with no anchor stays without one, by old source section.
// Every entry is a gap the re-check reports; none of them may be counted as
// verified against a current specification.
export const UNMAPPED = [
  [/^SES §四|^SES §五|^SES §九/, '会话规格不覆盖：层、运行时组与服务安装由多语言 M1、M2 与挂载设计规定'],
  [/^SES §十一/, '会话规格不覆盖：事件规则在事件协议规格'],
  [/^SES §十三/, '会话规格不覆盖：协议授权与诊断，部分明确不保证'],
  [/^CHN §连接器|^CHN §建立阶段错误|^CHN §共享监听器|^CHN §WebSocket 重连参数|^CHN §装饰器/, '连接层：0.19 改为 `rutis-host/dial` / `listen` 的行与连接插件，网络栈设计的注册与接入点部分已删'],
  [/^M1 /, '会话规格不覆盖：行、服务与运行时由多语言 M1 与挂载设计规定'],
  [/^SYN /, '多语言运行时设计的 0.9 基线，0.19 后未重写；实现以 M1、M2、挂载与 Rust 服务对外设计为准'],
  [/^REM /, '远程插件设计（0.17 前）已被 0.19 的远程会话设计取代；可用部分见 `design-remote-session-2026-10-28.md`'],
  [/^DIA /, '拨出设计定稿，但接入点、吊销与 `runtimes.remote` 已删；连接行为改由 `rutis-host/dial` 规定'],
  [/^APP /, '应用接入设计定稿未实现；0.19 改写 D2、D6'],
  [/^INS /, '实例服务设计（0.16 提案）；行服务与声明集合后来由 `design-declared-services-2026-10-29.md` 补充'],
]

// Rules that survive with a renamed or moved mechanism: the promise is still
// current, the words name an API 0.19 replaced. The re-check keeps the status
// and adds the note to the gap; it does not withdraw the row.
export const NOTES = new Map([
  ['CHN-10-10', '`Closer::replaced()` 已并入 `Closer::close(Close::Replaced)`（`design-remote-session-2026-10-28.md` §三）；4002 接管的行为仍现行'],
  ['CHN-2-9', '`Transport` 错误已并入 `ChannelError` / 连接插件的失败类别；通道结束与在途调用失败的行为仍现行'],
  ['CHN-2-16', '`Closed { reason }` 的映射已随 `Channel` 接口改名；行为仍现行'],
  ['REM-7-2', '`Transport` 类型已删，断链时在途调用失败一次的行为仍现行（登记表 `bridge/in-flight-call-fails-once`）'],
  ['bridge/dial-out-rules a', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；接受拨出、按 id 顶替的行为仍现行'],
  ['bridge/dial-out-rules e', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；断开先卸载行的行为仍现行'],
  ['bridge/dial-out-rules f', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；同 id 顶替的行为仍现行'],
  ['bridge/dial-out-rules g', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；停止不重连的行为仍现行'],
  ['SYN-9-26', '`sync-wait` 能力已删；“没有标记的同步调用不在检测范围内”仍是明确不保证，见 `SESS-40`'],
  ['REM-2-7', '`PeerRows` 已删（0.19）；“link 不查询 schema、不刷新行”仍现行'],
  ['REM-3-6', '导入导出资源已删；link 结束时清理它代装的行仍现行'],
  ['REM-3.2-8', '`Transport` / `Identity` / `Registration` 已删；link 停止时取消重试、关闭会话仍现行'],
  ['REM-3.2-10', '吊销已删；“一条 link 停止不关闭别的 link 用的共享物理连接”仍现行'],
  ['loader/peer-rows b', '节点行的 `export`、`import`、`events` 字段已删；同一 link 上的行共用一个会话仍现行'],
  ['events/old-peer-refused b', '0.18 及更早的对端仍在握手时被拒（`SESS-79`）；各语言的“被拒不重试”要按 0.19 的输出重抽'],
  ['CHN-8-3', '0.19 的接收授权由 `rutis-host/listen` 的 token 比对完成（`design-remote-session-2026-10-28.md` §六）；按验证得到的身份路由仍现行'],
  ['CHN-8-4', '0.19 的接收授权由 `rutis-host/listen` 的 token 比对完成（`design-remote-session-2026-10-28.md` §六）；拒绝无匹配、不建隐式 link 仍现行'],
  ['CHN-8-8', '吊销已删；断开现有连接靠停掉那一行，共享监听器不受影响仍现行'],
  ['DIA-6-3', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；4003 与“归为凭据被拒”仍现行'],
  ['DIA-6-12', '0.19 的输出行随连接插件改；分类与重试节奏仍现行，逐字输出待重抽'],
  ['DIA-6-13', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；10 秒问候时限仍现行'],
  ['DIA-6-14', '接入点在 0.19 删除（#695），监听改为 `rutis-host/listen` 行；4003 的迟到拒绝仍明确不保证'],
  ['NOD-1-4', '0.19 的 link 状态行改为连接插件的输出；类别、不带 token 仍现行'],
  ['host/check', '`peer:` 与远程运行时的提示已随 0.19 删除；check 的其余输出与退出码仍现行，文字待重抽'],
])
