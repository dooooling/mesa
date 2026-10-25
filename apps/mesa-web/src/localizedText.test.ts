// A1：LocalizedText 统一取值（优先 zh-CN，回落 default）的纯函数门禁。
//
// 这一层必须逐条钉死：取值规则一旦分叉，同一个 Descriptor 会在不同界面显示
// 不同语言（历史上只有事件编辑器读 zh-CN，资源选择器仍显示英文 default）。
import { describe, expect, it } from "vitest";
import { localizedText } from "./localizedText";

describe("localizedText（A1）", () => {
  it("有 zh-CN 时优先用 zh-CN", () => {
    expect(localizedText({ default: "NC Variable", "zh-CN": "NC 变量" })).toBe("NC 变量");
  });

  it("缺 zh-CN 时回落 default（老 Descriptor 不报错）", () => {
    expect(localizedText({ default: "Value" })).toBe("Value");
  });

  it("zh-CN 为空串/纯空白视为没有译文", () => {
    expect(localizedText({ default: "Value", "zh-CN": "" })).toBe("Value");
    expect(localizedText({ default: "Value", "zh-CN": "   " })).toBe("Value");
  });

  it("default 为空时用调用方兜底（通常是 id/key）", () => {
    expect(localizedText({ default: "" }, "register")).toBe("register");
    expect(localizedText(null, "register")).toBe("register");
    expect(localizedText(undefined)).toBe("");
  });
});
