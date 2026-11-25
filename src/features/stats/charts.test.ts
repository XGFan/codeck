import { describe, expect, it } from "vitest";
import type { DailyModelTokens, HourModelUsage } from "@/ipc/bindings";
import { themes, trendOption, usageBarOption } from "./charts";
import { buildHourlyUsageSeries, buildUsageSeries } from "./usage";

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

  it("单日 1h 柱子不可点，横轴只标 0/6/12/18 时，提示标题为「14 时」", () => {
    const dayStart = new Date("2026-11-24T00:00:00").getTime();
    const rows: HourModelUsage[] = [
      {
        startMs: dayStart + 14 * 3_600_000,
        model: "claude-opus-4-7",
        input: 100,
        output: 50,
        cacheRead: 0,
        cacheCreation: 0,
        cost: 3.5,
        requests: 2,
      },
    ];
    const usage = buildHourlyUsageSeries(rows, "cost", dayStart);
    const option = usageBarOption(usage, "cost", themes.light) as unknown as {
      series: { cursor: string }[];
      xAxis: { axisLabel: { interval: number; formatter: (v: string, idx: number) => string } };
      tooltip: {
        formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => string;
      };
    };
    expect(option.series[0]!.cursor).toBe("default");
    expect(option.xAxis.axisLabel.interval).toBe(0);
    expect(option.xAxis.axisLabel.formatter("0", 0)).toBe("0 时");
    expect(option.xAxis.axisLabel.formatter("6", 6)).toBe("6 时");
    expect(option.xAxis.axisLabel.formatter("7", 7)).toBe("");
    expect(option.xAxis.axisLabel.formatter("18", 18)).toBe("18 时");
    expect(option.tooltip.formatter([{ dataIndex: 14, marker: "", seriesName: "opus", value: 3.5 }])).toContain(
      "<b>14 时</b>",
    );
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

  it("单日走势横轴按 6 小时步进（0/6/12/18/24 时），无预估额度线", () => {
    const start = new Date("2026-11-24T00:00:00").getTime();
    const end = new Date("2026-11-25T00:00:00").getTime();
    const H = 3_600_000;
    const option = trendOption({
      rangeStartMs: start,
      rangeEndMs: end,
      currentPts: [
        [0, 0],
        [24 * H, 120],
      ],
      prevPts: [
        [0, 0],
        [24 * H, 100],
      ],
      labels: { current: "这一天", previous: "前一天", previousLegend: "前一天（11月23日 周一）" },
      nowElapsedMs: null,
      estimate: null,
      theme: themes.light,
      mode: "day",
    }) as unknown as {
      xAxis: { interval: number; axisLabel: { formatter: (v: number) => string } };
      series: { name: string; markLine?: unknown }[];
    };
    expect(option.xAxis.interval).toBe(6 * H);
    expect([0, 6, 12, 18, 24].map((h) => option.xAxis.axisLabel.formatter(h * H))).toEqual([
      "0 时",
      "6 时",
      "12 时",
      "18 时",
      "24 时",
    ]);
    expect(option.series[1]!.markLine).toBeUndefined();
  });
});
