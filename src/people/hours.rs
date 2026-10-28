//! Working hours: the weekly Mon–Sun × 0–23 grid, what it says about right
//! now in someone's own timezone, and the same week moved into yours.
//!
//! The arithmetic is the web app's (`lib/working-hours.ts`), so both say
//! the same thing about the same person: whole hours, and *today's* offsets
//! for both zones, so a week straddling a DST change never shows two shifts.

use std::collections::HashSet;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::api::models::WorkingHourCell;

pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const CELL: f32 = 13.;
const GAP: f32 = 2.;

/// Weekday (0 = Monday), hour and minute on the wall clock in `tz`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallClock {
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
}

pub fn zone(name: &str) -> Option<TimeZone> {
    TimeZone::get(name).ok()
}

/// The machine's own zone: whoever is looking is sitting at it.
pub fn local_zone_name() -> Option<String> {
    iana_time_zone::get_timezone().ok()
}

pub fn wall_clock(tz: &TimeZone, at: Timestamp) -> WallClock {
    let zoned = at.to_zoned(tz.clone());
    WallClock {
        weekday: zoned.weekday().to_monday_zero_offset() as u8,
        hour: zoned.hour() as u8,
        minute: zoned.minute() as u8,
    }
}

/// "Wed 14:05".
pub fn wall_clock_label(clock: WallClock) -> String {
    format!(
        "{} {:02}:{:02}",
        WEEKDAYS[clock.weekday as usize % 7],
        clock.hour,
        clock.minute
    )
}

/// Hours from `from` until the next marked hour, walking forward through
/// the week: 0 inside one, `None` when nothing is marked at all.
pub fn hours_until_working(cells: &[WorkingHourCell], from: WallClock) -> Option<u32> {
    let on: HashSet<WorkingHourCell> = cells.iter().copied().collect();
    (0..168u32).find(|step| {
        let slot = (from.weekday as u32 * 24 + from.hour as u32 + step) % 168;
        on.contains(&WorkingHourCell {
            weekday: (slot / 24) as u8,
            hour: (slot % 24) as u8,
        })
    })
}

/// Whether now is a good time, as far as the grid and the zone say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Inside a marked hour.
    Working,
    /// The next marked hour is this many hours away.
    BackIn(u32),
    /// Nothing marked.
    NoHours,
    /// Never opened an app, so the grid has no zone to be read in.
    NoTimezone,
}

/// Their wall clock and availability right now.
pub fn availability(
    timezone: Option<&str>,
    cells: &[WorkingHourCell],
    at: Timestamp,
) -> (Option<WallClock>, Availability) {
    let Some(tz) = timezone.and_then(zone) else {
        return (None, Availability::NoTimezone);
    };
    let clock = wall_clock(&tz, at);
    let state = match hours_until_working(cells, clock) {
        Some(0) => Availability::Working,
        Some(h) => Availability::BackIn(h),
        None => Availability::NoHours,
    };
    (Some(clock), state)
}

/// "back in 3h", "back in 2d 4h".
pub fn back_in(hours: u32) -> String {
    if hours < 24 {
        format!("back in {hours}h")
    } else if hours.is_multiple_of(24) {
        format!("back in {}d", hours / 24)
    } else {
        format!("back in {}d {}h", hours / 24, hours % 24)
    }
}

/// The week moved from `from`'s wall clock to `to`'s, to the nearest hour.
pub fn convert_week(
    cells: &[WorkingHourCell],
    from: &TimeZone,
    to: &TimeZone,
    at: Timestamp,
) -> Vec<WorkingHourCell> {
    let offset = |tz: &TimeZone| tz.to_offset(at).seconds() as i64;
    let shift = ((offset(to) - offset(from)) as f64 / 3600.).round() as i64;
    cells
        .iter()
        .map(|c| {
            let slot = (c.weekday as i64 * 24 + c.hour as i64 + shift).rem_euclid(168);
            WorkingHourCell {
                weekday: (slot / 24) as u8,
                hour: (slot % 24) as u8,
            }
        })
        .collect()
}

/// The grid, each square drawn by `cell(weekday, hour, marked)` so the
/// read-only and the editable grid share the layout and nothing else.
pub fn grid(
    cells: &[WorkingHourCell],
    cx: &App,
    cell: impl Fn(WorkingHourCell, bool) -> Stateful<Div>,
) -> Div {
    let theme = cx.theme();
    let on: HashSet<WorkingHourCell> = cells.iter().copied().collect();
    let label = |text: String| {
        div()
            .w(px(CELL))
            .text_size(px(9.))
            .text_color(theme.muted_foreground)
            .child(text)
    };
    v_flex()
        .gap(px(GAP))
        .child(
            h_flex()
                .gap(px(GAP))
                .child(div().w(px(30.)))
                .children((0..24u8).map(|h| {
                    label(if h % 3 == 0 {
                        h.to_string()
                    } else {
                        String::new()
                    })
                })),
        )
        .children((0..7u8).map(|weekday| {
            h_flex()
                .gap(px(GAP))
                .child(
                    div()
                        .w(px(30.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(WEEKDAYS[weekday as usize]),
                )
                .children((0..24u8).map(|hour| {
                    let c = WorkingHourCell { weekday, hour };
                    cell(c, on.contains(&c))
                }))
        }))
}

/// One square, coloured by whether it's marked.
pub fn square(id: impl Into<ElementId>, marked: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .size(px(CELL))
        .rounded(px(3.))
        .bg(if marked { theme.primary } else { theme.muted })
}

/// A read-only grid, for a colleague's hours.
pub fn read_only_grid(id: &str, cells: &[WorkingHourCell], cx: &App) -> Div {
    grid(cells, cx, |c, marked| {
        square(
            SharedString::from(format!("{id}-{}-{}", c.weekday, c.hour)),
            marked,
            cx,
        )
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(format!(
                "{} {:02}:00{}",
                WEEKDAYS[c.weekday as usize],
                c.hour,
                if marked { " — working" } else { "" }
            ))
            .build(window, cx)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(weekday: u8, hour: u8) -> WorkingHourCell {
        WorkingHourCell { weekday, hour }
    }

    fn at(weekday: u8, hour: u8) -> WallClock {
        WallClock {
            weekday,
            hour,
            minute: 0,
        }
    }

    #[::core::prelude::v1::test]
    fn hours_until_the_next_marked_hour() {
        let week = [cell(0, 9), cell(0, 10), cell(4, 17)];
        assert_eq!(hours_until_working(&week, at(0, 9)), Some(0));
        assert_eq!(hours_until_working(&week, at(0, 8)), Some(1));
        assert_eq!(hours_until_working(&week, at(0, 11)), Some(4 * 24 + 6));
        // Wraps from Sunday night into Monday.
        assert_eq!(hours_until_working(&week, at(6, 23)), Some(10));
        assert_eq!(hours_until_working(&[], at(0, 0)), None);
        assert_eq!(back_in(5), "back in 5h");
        assert_eq!(back_in(48), "back in 2d");
        assert_eq!(back_in(54), "back in 2d 6h");
    }

    #[::core::prelude::v1::test]
    fn availability_needs_a_zone() {
        let now: Timestamp = "2026-10-26T09:30:00Z".parse().unwrap();
        let week = [cell(0, 9)];
        assert_eq!(
            availability(Some("UTC"), &week, now).1,
            Availability::Working
        );
        assert_eq!(
            availability(None, &week, now),
            (None, Availability::NoTimezone)
        );
        assert_eq!(availability(Some("UTC"), &[], now).1, Availability::NoHours);
    }

    #[::core::prelude::v1::test]
    fn weeks_move_between_zones_and_wrap() {
        let utc = TimeZone::UTC;
        let tokyo = zone("Asia/Tokyo").unwrap();
        let now: Timestamp = "2026-10-27T12:00:00Z".parse().unwrap();
        // Tokyo is UTC+9: Sunday 20:00 UTC is Monday 05:00 there.
        assert_eq!(
            convert_week(&[cell(6, 20), cell(0, 1)], &utc, &tokyo, now),
            vec![cell(0, 5), cell(0, 10)]
        );
        // And back across Monday midnight to Sunday.
        assert_eq!(
            convert_week(&[cell(0, 5)], &tokyo, &utc, now),
            vec![cell(6, 20)]
        );
        assert_eq!(
            convert_week(&[cell(2, 9)], &utc, &utc, now),
            vec![cell(2, 9)]
        );
    }

    #[::core::prelude::v1::test]
    fn wall_clocks_count_from_monday() {
        // 2026-10-26 was a Monday.
        let monday: Timestamp = "2026-10-26T08:30:00Z".parse().unwrap();
        let clock = wall_clock(&zone("Europe/London").unwrap(), monday);
        assert_eq!(
            clock,
            WallClock {
                weekday: 0,
                hour: 8,
                minute: 30
            }
        );
        assert_eq!(wall_clock_label(clock), "Mon 08:30");
    }
}
