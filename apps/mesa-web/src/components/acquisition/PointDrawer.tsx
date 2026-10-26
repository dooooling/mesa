// 抽屉容器（通用组件，§2.8）：只负责容器、唯一说明与底部动作。
//
// 边界：**不做**任何校验、身份核验、生命周期或运行态判断——那些判断留在页面层，
// 本组件只接收 `open / busy / footer`。这样抽屉不会变成第二个业务分支点。
//
// 抽屉内**唯一**的说明句固定为 `DRAFT_NOTICE`（§4.3）：除它之外不再放教程、
// 流程说明或接口说明；错误/核验状态按各自时机由内容区给出。
// 只读抽屉（技术详情）传 `notice={null}` 即可不显示这句。
import type { ReactNode } from "react";
import { Drawer } from "antd";

/** 抽屉里的唯一说明（§4.3，全文不得再加第二句流程说明）。 */
export const DRAFT_NOTICE = "确认后加入页面草稿，保存配置后生效。";

export function PointDrawer({
  open,
  title,
  children,
  footer,
  onClose,
  busy,
  width = 640,
  notice,
  testId = "point-drawer",
}: {
  open: boolean;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  /** 保存中：禁止关闭抽屉（避免「请求还在飞、抽屉已关」的错位，§4.7）。 */
  busy?: boolean;
  width?: number;
  /** 说明行；`null` 表示不显示（只读抽屉）。 */
  notice?: string | null;
  testId?: string;
}) {
  const noticeText = notice === undefined ? DRAFT_NOTICE : notice;
  return (
    <Drawer
      open={open}
      title={title}
      width={width}
      onClose={onClose}
      closable={!busy}
      maskClosable={!busy}
      keyboard={!busy}
      destroyOnClose
      data-testid={testId}
      styles={{ body: { paddingTop: 8 }, footer: { padding: 12 } }}
      footer={footer ? <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>{footer}</div> : null}
    >
      {noticeText ? (
        <div
          data-testid="drawer-draft-notice"
          style={{ fontSize: 12, color: "#525252", marginBottom: 12 }}
        >
          {noticeText}
        </div>
      ) : null}
      {children}
    </Drawer>
  );
}
