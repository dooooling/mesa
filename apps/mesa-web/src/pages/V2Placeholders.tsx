// M1 占位页：总览/实时数据/事件/系统。M4 填总览+系统，M3 填顶层数据/事件。
// M1 只保证一级导航完整、路由可达，不复制旧页面内容。
import { Alert, Card } from "antd";

export function OverviewPage() {
  return (
    <Card size="small" title="总览">
      <Alert
        type="info"
        showIcon
        message="M4 实现"
        description="回答“现在有什么需要我处理”：系统状态 + 需要关注列表（设备/连接/数据健康/活动事件聚合，点击直达设备）。"
      />
    </Card>
  );
}

export function GlobalDataPage() {
  return (
    <Card size="small" title="实时数据">
      <Alert
        type="info"
        showIcon
        message="M3 实现"
        description="跨设备聚合搜索：设备/连接/状态过滤 + 全文搜索，行点击开 Drawer，可跳转所属设备。"
      />
    </Card>
  );
}

export function GlobalEventsPage() {
  return (
    <Card size="small" title="事件">
      <Alert
        type="info"
        showIcon
        message="M3 实现"
        description="全部设备事件聚合。当前全局事件仍在旧「事件」页可用，本页 M3 接入设备归属列与 Drawer。"
      />
    </Card>
  );
}

export function SystemPage() {
  return (
    <Card size="small" title="系统">
      <Alert
        type="info"
        showIcon
        message="M4 实现"
        description="运行状态 / Drivers / 存储 / 事件服务 / 版本 / 诊断（平台级概念收敛到此，不与设备操作混在一起）。"
      />
    </Card>
  );
}
