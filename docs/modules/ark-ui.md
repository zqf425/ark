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

`arkd` 在装配阶段创建一个 `UiBus`，授予本构建实际具备的能力
（`Tool`、`ModelProvider`、`UiPage`、`UiPanel`），再以 `ark.builtin.code-ui` 注册 Code 页和四个面板，
最后以 `BaseServices::with_ui` 注入 `ark-server`。注册返回的句柄在装配处 `forget()`：句柄一旦 drop
就会撤销注册，因此它必须活到进程结束。

`GET /v1/runtime/capabilities` 的 `capabilities` 与 `catalog` 两个字段都从该总线读取，
`ark-server` 不再保留任何硬编码能力清单。未装配总线时该路由报告空能力与空 catalog，不宣称
`ui_page`——否则渲染层会期待一份无人能产出的导航。

`UiCapability` 因此包含 `capabilities()` 与 `catalog_json()` 两个方法；后者是
`UiCatalog::to_json` 的端口投影，保证 HTTP 响应与渲染导航来自同一次序列化。

Code UI 当前以内置注册项存在于本 crate；独立 `plugin-code-ui` 是计划目标，实施前不得把该目录
当成现有代码结构。
