// 共享秒级时钟：实时页“更新”列的唯一时间源。
// 单 interval，多 AgeCell 订阅；每 Cell 独立 interval 是行数灾难。
// tick 变化只重渲染消费它的 AgeCell（叶子 span），不碰整表。
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

const StaleTickContext = createContext(0);

export function StaleClockProvider({ children }: { children: ReactNode }) {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => {
      setTick((n) => n + 1);
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);
  return <StaleTickContext.Provider value={tick}>{children}</StaleTickContext.Provider>;
}

/** 当前秒 tick（AgeCell 重算年龄的触发器）。 */
export function useStaleTick(): number {
  return useContext(StaleTickContext);
}
