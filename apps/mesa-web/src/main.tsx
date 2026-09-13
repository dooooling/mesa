import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { ConfigProvider, theme } from "antd";
import "./carbon.css";
// IBM Plex 随 bundle 本地提供（@fontsource/SIL OFL）：Sans 取 300（展示字重，
// 品牌嗓音）/400/600，Mono 取 400/500。只引需要的字重，不整包拉取。
import "@fontsource/ibm-plex-sans/300.css";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import { MONO, SANS } from "./theme";
import App from "./App";

// Carbon token 映射（web-ui/DESIGN.md）：单 IBM 蓝 #0f62fe、全 0 圆角、
// 白画布 + hairline #e0e0e0、墨黑正文 + 次级灰。CSS 表达不了的输入框
// 形态/侧边选中态见 carbon.css；行为逻辑与此无关。
const carbonTokens = {
  colorPrimary: "#0f62fe",
  colorPrimaryHover: "#0043ce",
  colorPrimaryActive: "#002d9c",
  colorLink: "#0f62fe",
  colorLinkHover: "#0043ce",
  colorLinkActive: "#002d9c",
  colorSuccess: "#24a148",
  colorError: "#da1e28",
  colorInfo: "#0f62fe",
  colorBgLayout: "#ffffff",
  colorBgContainer: "#ffffff",
  colorBorder: "#e0e0e0",
  colorBorderSecondary: "#e0e0e0",
  colorText: "#161616",
  colorTextSecondary: "#525252",
  colorTextTertiary: "#8c8c8c",
  fontFamily: SANS,
  fontFamilyCode: MONO,
  borderRadius: 0,
  borderRadiusLG: 0,
  borderRadiusSM: 0,
  borderRadiusXS: 0,
  borderRadiusOuter: 0,
  boxShadow: "none",
  boxShadowSecondary: "none",
  boxShadowTertiary: "none",
  controlOutlineWidth: 0,
};

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ConfigProvider
      theme={{
        algorithm: theme.defaultAlgorithm,
        token: carbonTokens,
        components: {
          Input: { colorBgContainer: "#f4f4f4", colorBorder: "#8d8d8d", activeBorderColor: "#0f62fe", hoverBorderColor: "#0f62fe" },
          InputNumber: { colorBgContainer: "#f4f4f4", colorBorder: "#8d8d8d", activeBorderColor: "#0f62fe", hoverBorderColor: "#0f62fe" },
          Select: { colorBgContainer: "#f4f4f4", colorBorder: "#8d8d8d", optionSelectedBg: "#e5e5e5", optionSelectedColor: "#161616" },
          Menu: { itemBg: "#ffffff", itemColor: "#161616", itemHoverBg: "#f4f4f4", itemHoverColor: "#161616", itemSelectedBg: "#e5e5e5", itemSelectedColor: "#161616" },
          Table: { headerBg: "#f4f4f4", headerColor: "#161616", borderColor: "#e0e0e0", rowHoverBg: "#f4f4f4" },
          Button: { primaryShadow: "none", dangerShadow: "none" },
          Card: { colorBorderSecondary: "#e0e0e0" },
        },
      }}
    >
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </ConfigProvider>
  </React.StrictMode>,
);
