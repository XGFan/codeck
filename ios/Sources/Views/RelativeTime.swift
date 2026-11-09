import Foundation

/// 刚刚 / N 分钟前 / N 小时前 / 昨天 / 周X（一周内）/ M月D日 — same as `formatRelative` in src/lib/format.ts.
enum RelativeTime {
    static func format(_ date: Date, now: Date = .now, calendar: Calendar = .current) -> String {
        let mins = Int(now.timeIntervalSince(date) / 60)
        if mins < 1 { return "刚刚" }
        if mins < 60 { return "\(mins) 分钟前" }
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day ?? 0
        if days == 0 { return "\(mins / 60) 小时前" }
        if days == 1 { return "昨天" }
        if days < 7 {
            let weekday = calendar.component(.weekday, from: date)
            return "周" + ["日", "一", "二", "三", "四", "五", "六"][weekday - 1]
        }
        let c = calendar.dateComponents([.month, .day], from: date)
        return "\(c.month ?? 0)月\(c.day ?? 0)日"
    }
}
