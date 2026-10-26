// 两类向导流程共享同一个真实路由入口；按独立流程分文件，便于 CI 文件级分片。
import { render } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { AddDeviceFlow } from "../addDevice/AddDeviceFlow";

export function renderAddDeviceFlow() {
  return render(
    <MemoryRouter initialEntries={["/devices/new"]}>
      <Routes>
        <Route path="/devices/new" element={<AddDeviceFlow />} />
      </Routes>
    </MemoryRouter>,
  );
}
