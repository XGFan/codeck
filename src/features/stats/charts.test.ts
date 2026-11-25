import { describe, expect, it } from "vitest";
import type { DailyModelTokens } from "@/ipc/bindings";
import { themes, trendOption, usageBarOption } from "./charts";
import { buildUsageSeries } from "./usage";

const row = (day: string, model: string, cost: number): DailyModelTokens => ({
  day,
  model,
  input: 1000,
  output: 1000,
  cacheRead: 0,
  cacheCreation: 0,
  cost,
  requests: 5,
});

describe("usageBarOption", () => {
  it("按模型族堆叠（fable 排在 opus 前），纵轴与悬停提示显示美元", () => {
    const usage = buildUsageSeries(
      [
        row("2026-10-20", "claude-opus-4-7", 1.5),
        row("2026-10-20", "claude-opus-4-8", 0.5),
        row("2026-10-21", "claude-sonnet-5-5", 3),
        row("2026-10-21", "claude-fable-5-1", 4),
        row("2026-10-21", "no-price", 0),
      ],
      "cost",
      {
        fromMs: new Date("2026-10-20T00:00:00").getTime(),
        toMs: new Date("2026-10-21T23:59:59").getTime(),
      },
      new Date("2026-10-21T12:00:00").getTime(),
    );
    const option = usageBarOption(usage, "cost", themes.light) as unknown as {
      series: { name: string; data: number[]; cursor: string }[];
      yAxis: { axisLabel: { formatter: (v: number) => string } };
      tooltip: {
        formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => string;
      };
    };
    expect(option.series.map((s) => [s.name, s.data])).toEqual([
      ["fable", [0, 4]],
      ["opus", [2, 0]],
      ["sonnet", [0, 3]],
    ]);
    expect(option.series[0]!.cursor).toBe("pointer");
    expect(option.yAxis.axisLabel.formatter(20)).toBe("$20");
    expect(option.tooltip.formatter([{ dataIndex: 0, marker: "", seriesName: "opus", value: 2 }])).toContain("$2.00");
  });
});

describe("trendOption", () => {
  it("纵轴上限覆盖预估额度，「现在」横排放在竖线顶端右侧，预估额度在线上方靠左", () => {
    const start = new Date("2026-11-23T14:00:00").getTime();
    const end = start + 7 * 86_400_000;
    const option = trendOption({
      rangeStartMs: start,
      rangeEndMs: end,
      currentPts: [
        [0, 0],
        [86_400_000, 85.26],
      ],
      prevPts: [
        [0, 0],
        [end - start, 190],
      ],
      labels: { current: "本周期", previous: "上一周期" },
      nowElapsedMs: 86_400_000,
      estimate: {
        estimatedCost: 275.03,
        weeklyPercent: 31,
        lineLabel: "预估额度 $275.03",
        legendText: "预估额度 = 已用 Cost ÷ 本周已用 31%",
      },
      theme: themes.light,
    }) as unknown as {
      yAxis: { max: number };
      series: {
        name: string;
        markLine?: {
          data: {
            xAxis?: number;
            yAxis?: number;
            label: { formatter: string; position: string; align?: string; verticalAlign?: string };
          }[];
        };
      }[];
    };
    expect(option.yAxis.max).toBeGreaterThanOrEqual(275.03);
    expect(option.series.map((s) => s.name)).toEqual(["上一周期", "本周期"]);
    const lines = option.series[1]!.markLine!.data;
    expect(lines).toHaveLength(2);
    expect(lines[0]!.label).toMatchObject({
      formatter: "现在",
      position: "end",
      align: "left",
      verticalAlign: "top",
    });
    expect(lines[1]!.yAxis).toBe(275.03);
    expect(lines[1]!.label).toMatchObject({
      formatter: "预估额度 $275.03",
      position: "insideStartTop",
    });
  });
});
