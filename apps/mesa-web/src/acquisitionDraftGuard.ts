// 采集草稿的生命周期守卫（§7-7 定案）：切换连接、离开页面要提示；刷新用浏览器
// 原生离开提醒；**不静默丢弃**。
//
// 为什么单独一个模块：这些提示横跨页面层（采集窗格）与外壳层（设备 Tab / 连接选择），
// 而它们都不是「校验」——校验仍在页面层与既有模型里，这里只负责「别让用户白改」。
import { useEffect } from "react";
import { Modal } from "antd";

/** 原生 beforeunload 提示文案（浏览器只会显示自己的统一措辞，这里用于测试锚定）。 */
export const DRAFT_LEAVE_MESSAGE = "当前有未保存的采集配置修改，离开会丢失。";

/**
 * 注册浏览器原生离开提醒（刷新 / 关闭标签页 / 地址栏跳转）。
 *
 * 只做注册与注销，不做任何判断：`dirty` 由调用方（页面层）给出。
 * 无修改时不注册，避免正常的页面刷新被拦住。
 */
export function useBeforeUnloadGuard(dirty: boolean): void {
  useEffect(() => {
    if (!dirty) return;
    const handler = (event: BeforeUnloadEvent) => {
      // 现代浏览器忽略自定义文案，但仍必须 preventDefault + returnValue 才会弹。
      event.preventDefault();
      event.returnValue = DRAFT_LEAVE_MESSAGE;
      return DRAFT_LEAVE_MESSAGE;
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [dirty]);
}

/**
 * 应用内跳转前的确认（切换连接 / 切换 Tab）：有未保存修改时问一次。
 *
 * 返回 `true` 表示用户确认放弃修改。没修改时**不弹**任何框。
 */
export function confirmDiscardDraft(dirty: boolean, action: string): Promise<boolean> {
  if (!dirty) return Promise.resolve(true);
  return new Promise((resolve) => {
    Modal.confirm({
      title: `${action}会丢弃当前未保存的采集配置修改，确定继续？`,
      content: DRAFT_LEAVE_MESSAGE,
      okText: "放弃修改并继续",
      cancelText: "继续编辑",
      onOk: () => resolve(true),
      onCancel: () => resolve(false),
    });
  });
}
