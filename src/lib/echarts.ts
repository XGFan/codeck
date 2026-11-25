import { BarChart, LineChart } from "echarts/charts";
import { GridComponent, MarkAreaComponent, MarkLineComponent, TooltipComponent } from "echarts/components";
import * as echarts from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";

echarts.use([
  BarChart,
  LineChart,
  GridComponent,
  MarkAreaComponent,
  MarkLineComponent,
  TooltipComponent,
  CanvasRenderer,
]);

export type ChartOption = echarts.EChartsCoreOption;

export interface ChartHandle {
  setOption(option: ChartOption): void;
  /** Fires with the clicked data item's `params` (echarts click event). */
  onClick(handler: (params: unknown) => void): void;
  dispose(): void;
}

/** Tree-shaken chart bound to `el`: auto-resizes with the element, `dispose` releases everything. */
export function createChart(el: HTMLElement): ChartHandle {
  const chart = echarts.init(el, undefined, { renderer: "canvas" });
  // Resize on the next frame: resizing inside the observer callback re-triggers it ("ResizeObserver loop").
  let frame = 0;
  const ro = new ResizeObserver(() => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => chart.resize());
  });
  ro.observe(el);
  return {
    setOption: (option) => chart.setOption(option, true),
    onClick: (handler) => {
      chart.off("click");
      chart.on("click", handler);
    },
    dispose: () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      chart.dispose();
    },
  };
}
