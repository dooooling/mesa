// Endpoint Workspace（P1-4）：Endpoint 的专属工作空间，严格归属 Device
//（路由 /devices/:deviceId/endpoints/:endpointId）。DeviceDetail 只管
// Device 身份与连接列表；连接的编辑/采集/事件/诊断全部收敛到这里的五个 tab。
import { useEffect, useState } from "react";
import { Breadcrumb, Button, Card, Descriptions, Space, Tabs, Tag, message } from "antd";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api } from "../api";
import type { Device } from "../deviceModel";
import { isRunningState } from "../deviceModel";
import { EndpointAcquisitionPane } from "../components/EndpointAcquisitionPane";
import { EndpointConnectionPane } from "../components/EndpointConnectionPane";
import { EventTaskEditor } from "../components/EventTaskEditor";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface EndpointDetail {
  id: string;
  name?: string;
  driver_id: string;
  device_id?: string;
  state?: string;
}

export function EndpointWorkspacePage() {
  const { deviceId = "", endpointId = "" } = useParams();
  const nav = useNavigate();
  const [device, setDevice] = useState<Device | null>(null);
  const [ep, setEp] = useState<EndpointDetail | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [taskCount, setTaskCount] = useState<number | null>(null);
  const [eventTaskCount, setEventTaskCount] = useState<number | null>(null);
  const [diag, setDiag] = useState<unknown>(null);
  const [tab, setTab] = useState("overview");

  const load = async () => {
    try {
      const d = (await api.getDevice(deviceId)) as Device;
      setDevice(d);
    } catch {
      // 设备名拿不到不致命，面包屑回落 id
    }
    try {
      const r = (await fetch(`/api/v1/endpoints/${endpointId}`).then((x) => {
        if (x.status === 404) throw Object.assign(new Error("not found"), { status: 404 });
        return x.json();
      })) as EndpointDetail & { connection?: unknown };
      const list = (await api.listEndpoints()) as { endpoints?: Array<{ id: string; runtime?: { state?: string }; state?: string }> };
      const live = (list.endpoints ?? []).find((e) => e.id === endpointId);
      setEp({
        id: r.id ?? endpointId,
        name: r.name ?? r.id ?? endpointId,
        driver_id: r.driver_id,
        device_id: r.device_id,
        state: live?.state ?? live?.runtime?.state,
      });
      setNotFound(false);
    } catch (e) {
      if ((e as { status?: number })?.status === 404) setNotFound(true);
      else message.error("加载连接失败");
    }
    fetch(`/api/v1/tasks?endpoint=${endpointId}`)
      .then((x) => x.json())
      .then((j) => setTaskCount((j.tasks ?? []).length))
      .catch(() => setTaskCount(null));
    api
      .listEventTasks(endpointId)
      .then((j) => setEventTaskCount((j.event_tasks ?? []).length))
      .catch(() => setEventTaskCount(null));
  };
  useEffect(() => { load(); }, [deviceId, endpointId]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadDiag = async () => {
    try {
      setDiag(await api.endpointDiagnostics(endpointId));
    } catch (e) {
      message.error((e as Error)?.message ?? "诊断加载失败");
    }
  };

  const act = async (a: "start" | "stop" | "delete") => {
    if (a === "delete") {
      await api.stopEndpoint(endpointId).catch(() => {});
      await sleep(300);
      const r = await api.deleteEndpoint(endpointId);
      if (r.status !== 200) {
        message.error(r.body?.error?.message ?? "删除失败");
        return;
      }
      message.success("已删除连接，所属设备保留");
      nav(`/devices/${deviceId}`);
      return;
    }
    const r = a === "stop" ? await api.stopEndpoint(endpointId) : await api.startEndpoint(endpointId);
    if (r.status !== 200) {
      message.error(r.body?.error?.message ?? (a === "stop" ? "停止失败" : "启动失败"));
      load();
      return;
    }
    message.success(a === "stop" ? "已停止" : "已启动");
    load();
  };

  if (notFound) {
    return (
      <Card size="small" title="连接不存在">
        <p style={{ color: "#525252" }}>连接 `{endpointId}` 不存在，可能已被删除。</p>
        <Button type="primary" onClick={() => nav(`/devices/${deviceId}`)}>返回设备</Button>
      </Card>
    );
  }

  const running = isRunningState(ep?.state);
  const mismatch = ep?.device_id && ep.device_id !== deviceId;

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Breadcrumb
        items={[
          { title: <Link to="/devices">设备</Link> },
          { title: <Link to={`/devices/${deviceId}`}>{device?.name ?? deviceId}</Link> },
          { title: ep?.name ?? endpointId },
        ]}
      />

      {mismatch ? (
        <Card size="small">
          <span style={{ fontSize: 12, color: "#525252" }}>
            该连接实际归属设备 {ep?.device_id}，<Link to={`/devices/${ep?.device_id}/endpoints/${endpointId}`}>前往正确位置 →</Link>
          </span>
        </Card>
      ) : null}

      <Card
        size="small"
        title={
          <Space>
            <span>{ep?.name ?? endpointId}</span>
            {ep ? <Tag>{ep.driver_id}</Tag> : null}
            <Tag color={running ? "green" : (ep?.state ?? "").toUpperCase() === "FAILED" ? "red" : "default"}>
              {ep?.state ?? "—"}
            </Tag>
          </Space>
        }
        extra={
          <Space>
            {!running
              ? <Button size="small" type="primary" onClick={() => act("start")}>启动</Button>
              : <Button size="small" onClick={() => act("stop")}>停止</Button>}
            <Button size="small" danger onClick={() => act("delete")}>删除</Button>
          </Space>
        }
      >
        <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, color: "#525252" }}>
          {endpointId}
        </span>
      </Card>

      <Card size="small">
        <Tabs
          activeKey={tab}
          onChange={(k) => {
            setTab(k);
            if (k === "diagnostics" && diag === null) loadDiag();
          }}
          items={[
            {
              key: "overview",
              label: "概览",
              children: (
                <Descriptions size="small" column={1} bordered style={{ maxWidth: 640 }}>
                  <Descriptions.Item label="名称">{ep?.name ?? endpointId}</Descriptions.Item>
                  <Descriptions.Item label="驱动">{ep?.driver_id ?? "—"}</Descriptions.Item>
                  <Descriptions.Item label="所属设备">{device?.name ?? deviceId}</Descriptions.Item>
                  <Descriptions.Item label="运行态">{ep?.state ?? "—"}</Descriptions.Item>
                  <Descriptions.Item label="采集任务">{taskCount ?? "—"}</Descriptions.Item>
                  <Descriptions.Item label="事件订阅">{eventTaskCount ?? "—"}</Descriptions.Item>
                </Descriptions>
              ),
            },
            {
              key: "connection",
              label: "连接",
              children: ep ? (
                <EndpointConnectionPane
                  endpointId={endpointId}
                  deviceId={ep.device_id ?? deviceId}
                  driverId={ep.driver_id}
                  initialName={ep.name ?? endpointId}
                  onChanged={load}
                />
              ) : (
                <div style={{ color: "#525252" }}>加载中…</div>
              ),
            },
            {
              key: "acquisition",
              label: "采集",
              children: ep ? (
                <EndpointAcquisitionPane endpointId={endpointId} driverId={ep.driver_id} onChanged={load} />
              ) : (
                <div style={{ color: "#525252" }}>加载中…</div>
              ),
            },
            {
              key: "events",
              label: "事件",
              children: <EventTaskEditor fixedEndpointId={endpointId} />,
            },
            {
              key: "diagnostics",
              label: "诊断",
              children: diag ? (
                <pre style={{ fontSize: 12, background: "#f4f4f4", padding: 12, overflow: "auto" }}>
                  {JSON.stringify(diag, null, 2)}
                </pre>
              ) : (
                <div style={{ color: "#525252", fontSize: 12 }}>加载诊断中…</div>
              ),
            },
          ]}
        />
      </Card>
    </div>
  );
}
