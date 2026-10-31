# ark-ui 模块设计

## 职责与边界

`ark-ui` 是页面和面板的注册总线，实现 `UiCapability`。它维护路由、导航和能力门禁的单一 catalog；
页面通过注册获得句柄，句柄 drop 后自动撤销，避免遗留死路由。

## 约束

- 页面 ID 和 route 全局唯一，重复为硬错误。
- catalog 同时驱动运行时能力接口、渲染导航和 shell 路由；禁止维护第二份导航表。
- 页面不直接访问运行时裸 HTTP，而使用由契约生成的 typed client；主题、鉴权、错误边界和事件桥由
  宿主提供。

## 当前实现

`arkd` 在装配阶段创建一个 `UiBus`，以 `ark.builtin.code-ui` 注册 Code 页和四个面板，最后以
`BaseServices::with_ui` 注入 `ark-server`。注册返回的句柄在装配处 `forget()`：句柄一旦 drop
就会撤销注册，因此它必须活到进程结束。组合根**不再**声明能力清单。

能力按宿主归属分别派生，不再由任何一份平行清单声明：

- `ui_page` / `ui_panel` 由 `UiBus` 从实际注册结果派生（有页才有 `ui_page`，有面板才有 `ui_panel`）；
- `tool` 由工具注册表派生，`model_provider` 由 provider 表派生，二者在 `ark-server` 的 handler
  中合成。

这样消除了「wiring 必须记得 grant」的第二事实源：漏授不再可能让路由反方向说谎。

`GET /v1/runtime/capabilities` 返回 `ark-contract` 的 `CapabilitiesResponse`
（`version`、`capabilities`、`catalog`），其中 `catalog` 是 `UiCatalog`（`pages` + `PanelEntry`），
已随 ts_rs 导出到 `web/src/generated`。未装配总线时该路由报告空能力与空 catalog，不宣称
`ui_page`——否则渲染层会期待一份无人能产出的导航。

`UiCapability` 因此包含 `capabilities()` 与 `catalog(available)` 两个方法。后者接收调用方的
完整能力集合，因为页面可能要求本总线不拥有的能力（如 `tool`）；只按总线自身能力过滤会隐藏
运行时实际可服务的页面。catalog 会按 `required_caps` 过滤，不满足条件的页/面板不进入响应，
这与「隐藏而非展示为损坏」的约定一致。

Code UI 当前以内置注册项存在于本 crate；独立 `plugin-code-ui` 是计划目标，实施前不得把该目录
当成现有代码结构。
