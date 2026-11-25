import { useEffect, useRef } from "react";
import { type ChartHandle, type ChartOption, createChart } from "@/lib/echarts";
import { useTheme } from "@/lib/theme";

/** Follows the app theme (src/lib/theme.ts). */
export const useDark = (): boolean => useTheme() === "dark";

interface Props {
  option: ChartOption;
  height: number;
  label: string;
  testId?: string;
  onPick?: (params: { data?: unknown; name?: string; seriesName?: string; dataIndex?: number }) => void;
}

export function Chart({ option, height, label, testId, onPick }: Props) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<ChartHandle | null>(null);
  const pick = useRef(onPick);
  pick.current = onPick;

  useEffect(() => {
    const c = createChart(el.current!);
    chart.current = c;
    c.onClick((p) => pick.current?.(p as never));
    return () => {
      c.dispose();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    chart.current?.setOption(option);
  }, [option]);

  return <div ref={el} role="img" aria-label={label} data-testid={testId} style={{ height, width: "100%" }} />;
}
