# 长期 memory 存储

## 当前实现

ark 为用户显式维护、可跨会话复用的长期偏好或事实提供 JSON 存储。HTTP 表面为：

- `GET /v1/memory`：列表；
- `POST /v1/memory`：新增；
- `PATCH /v1/memory/{id}`：更新；
- `DELETE /v1/memory/{id}`：删除；
- `GET /v1/memory/diagnostics`：返回文件路径、条目数与最后写入时间。

数据保存在 `<data_dir>/memory.json`，原子写入并采用 UTF-8 无 BOM。文件缺失或损坏时以空集合启动，
从而不阻断服务启动；下一次成功写入会建立有效文件。文本不得为空且最大为 16 KiB UTF-8 字节。未知 id
更新或删除返回 `not_found`，校验失败返回 `validation_error`。

## 已批准计划

无。

## 待决

- 是否对损坏的 `memory.json` 保留备份及暴露诊断告警；
- 自动抽取、检索排序、用户范围与向模型 prompt 注入的策略；
- tags/source 的上限、索引和搜索语义。
