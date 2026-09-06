// Vitest 全局 setup：补齐 jsdom 缺失的浏览器 API（antd 渲染需要）。
// NOTE: 本工程未开 vitest globals，testing-library 的自动 cleanup 不会注册，
// 这里显式注册，保证每个测试后卸载组件，避免跨测试 DOM 残留造成多匹配。
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(() => {
  cleanup();
});

if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

if (typeof window !== "undefined" && !window.ResizeObserver) {
  window.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof window.ResizeObserver;
}
