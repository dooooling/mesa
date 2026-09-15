// 未知路由：诚实 404（不再回总览，避免错误 URL 被静默吞掉）。
import { Button, Result } from "antd";
import { useNavigate } from "react-router-dom";

export function NotFoundPage() {
  const nav = useNavigate();
  return (
    <Result
      status="404"
      title="404"
      subTitle="页面不存在，可能已被删除或链接有误。"
      extra={
        <Button type="primary" onClick={() => nav("/overview")}>
          返回总览
        </Button>
      }
    />
  );
}
