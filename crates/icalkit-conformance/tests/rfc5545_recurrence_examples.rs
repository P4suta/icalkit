// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Every worked recurrence example RFC 5545 section 3.8.5.3 prints, with the expected column
//! taken from the RFC and not from what this implementation returns.
//!
//! Section 3.8.5.3 is the only place the specification answers its own recurrence questions.
//! Thirty-seven of its examples name whole days and five name clock readings inside a day; all
//! forty-two are below, in the order the RFC prints them, each keeping the RFC's own heading as
//! its name so a reviewer can hold the two side by side. Where the RFC prints an open-ended
//! series ("...") the window is drawn around exactly what it printed, so every row is an
//! equality and never a prefix check.
//!
//! # The one convention this file had to choose
//!
//! The RFC states every example in `America/New_York` and switches between EDT and EST inside
//! several of the answers. This crate never resolves a zone — `docs/adr/0003` puts that at the
//! caller — so a `TZID` is not something an input here can carry. The wall clock the RFC prints
//! is therefore read onto the UTC timeline unchanged, for `DTSTART`, for `UNTIL`, and for the
//! answer. What that preserves is the whole of what the examples are about: which days a rule
//! names, which clock readings inside a day, how `COUNT` runs out and where `UNTIL` stops. What
//! it drops is the EDT/EST label the RFC prints beside each group, which is a fact about the
//! zone the caller applied and not about the rule.
//!
//! One example is only reproducible under that reading, and it is worth naming rather than
//! hiding: `FREQ=HOURLY;INTERVAL=3;UNTIL=19970902T170000Z` from a 9:00 AM EDT `DTSTART` is
//! printed as 09:00, 12:00 and 15:00. Read as a true UTC instant the `UNTIL` is 13:00 EDT and
//! only the first of the three survives, so the RFC's own answer is the wall-clock reading. A
//! caller that resolves `America/New_York` properly gets the arithmetic the RFC wrote down
//! rather than the answer it printed; this file asserts the answer it printed.
//!
//! # What this file is entitled to conclude
//!
//! A disagreement here is a defect in the engine, because the right-hand column is the
//! specification's. The corpus is deliberately assembled through the public surface only —
//! `parse_recur` reads the `RECUR` value the RFC prints, `RecurrenceInput` carries `DTSTART`
//! and any `EXDATE`, and `RecurrenceInput::search` produces the answer — so a shape that cannot
//! be reached from outside `ical-recur` is a finding about the surface as much as about the
//! engine.

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Instant, Limits, Meter,
    Severity, UtcOffset,
};
use icalkit_conformance::internal::recur::{
    OverrideSet, RecurrenceInput, SearchStep, ValueKind, Window, parse_recur,
};

/// The hour every example in section 3.8.5.3 that names whole days starts at.
const NINE_AM: (u8, u8) = (9, 0);

/// How the RFC prints the days of one month in an answer.
///
/// Three forms, because the RFC uses three: an inclusive range ("January 1-31"), an arithmetic
/// run it elides ("September 2,4,6,8...24,26,28,30"), and a plain list. Transcribing each in
/// the shape the RFC wrote it keeps this table checkable by eye against the document.
#[derive(Clone, Copy, Debug)]
enum Days {
    /// An inclusive range of days, both ends printed.
    Range(u8, u8),
    /// An arithmetic run: first, last, and the step between them.
    Step(u8, u8, u8),
    /// The days as the RFC lists them.
    List(&'static [u8]),
}

impl Days {
    /// The days named, ascending.
    fn days(self) -> Vec<u8> {
        match self {
            Self::Range(first, last) => (first..=last).collect(),
            Self::Step(first, last, step) => (first..=last).step_by(usize::from(step)).collect(),
            Self::List(days) => days.to_vec(),
        }
    }
}

/// One example whose answer names whole days, all of them at 9:00 AM.
#[derive(Clone, Copy, Debug)]
struct DayCase {
    /// The RFC's own heading for the example.
    name: &'static str,
    /// `DTSTART`, as year, month and day.
    dtstart: (u16, u8, u8),
    /// The `RECUR` value, transcribed with the RFC's folds joined.
    rule: &'static str,
    /// The `EXDATE` the example carries, where it carries one.
    exdates: &'static [(u16, u8, u8)],
    /// The half-open window, drawn around exactly what the RFC printed.
    window: ((u16, u8, u8), (u16, u8, u8)),
    /// The answer, month by month, in the RFC's order.
    expected: &'static [(u16, u8, Days)],
}

/// One example whose answer names clock readings inside one or more days.
#[derive(Clone, Copy, Debug)]
struct ClockCase {
    /// The RFC's own heading for the example.
    name: &'static str,
    /// `DTSTART`, as year, month, day, hour and minute.
    dtstart: (u16, u8, u8, u8, u8),
    /// The `RECUR` value, transcribed with the RFC's folds joined.
    rule: &'static str,
    /// The half-open window, drawn around exactly what the RFC printed.
    window: ((u16, u8, u8), (u16, u8, u8)),
    /// The days the answer covers, in order.
    days: &'static [(u16, u8, u8)],
    /// The clock readings inside each of those days, in order.
    times: &'static [(u8, u8)],
}

/// The eight hours and three minutes "every 20 minutes from 9:00 AM to 4:40 PM" names.
const EVERY_TWENTY: &[(u8, u8)] = &[
    (9, 0),
    (9, 20),
    (9, 40),
    (10, 0),
    (10, 20),
    (10, 40),
    (11, 0),
    (11, 20),
    (11, 40),
    (12, 0),
    (12, 20),
    (12, 40),
    (13, 0),
    (13, 20),
    (13, 40),
    (14, 0),
    (14, 20),
    (14, 40),
    (15, 0),
    (15, 20),
    (15, 40),
    (16, 0),
    (16, 20),
    (16, 40),
];

/// Section 3.8.5.3's examples that name whole days, in the order the RFC prints them.
const DAY_CASES: &[DayCase] = &[
    DayCase {
        name: "Daily for 10 occurrences",
        dtstart: (1997, 9, 2),
        rule: "FREQ=DAILY;COUNT=10",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 10, 1)),
        expected: &[(1997, 9, Days::Range(2, 11))],
    },
    DayCase {
        name: "Daily until December 24, 1997",
        dtstart: (1997, 9, 2),
        rule: "FREQ=DAILY;UNTIL=19971224T000000Z",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 1, 1)),
        expected: &[
            (1997, 9, Days::Range(2, 30)),
            (1997, 10, Days::Range(1, 31)),
            (1997, 11, Days::Range(1, 30)),
            (1997, 12, Days::Range(1, 23)),
        ],
    },
    DayCase {
        name: "Every other day - forever",
        dtstart: (1997, 9, 2),
        rule: "FREQ=DAILY;INTERVAL=2",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 11, 1)),
        expected: &[
            (1997, 9, Days::Step(2, 30, 2)),
            (1997, 10, Days::Step(2, 30, 2)),
        ],
    },
    DayCase {
        name: "Every 10 days, 5 occurrences",
        dtstart: (1997, 9, 2),
        rule: "FREQ=DAILY;INTERVAL=10;COUNT=5",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 11, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 12, 22])),
            (1997, 10, Days::List(&[2, 12])),
        ],
    },
    DayCase {
        name: "Every day in January, for 3 years (the YEARLY spelling)",
        dtstart: (1998, 1, 1),
        rule: "FREQ=YEARLY;UNTIL=20000131T140000Z;BYMONTH=1;BYDAY=SU,MO,TU,WE,TH,FR,SA",
        exdates: &[],
        window: ((1998, 1, 1), (2000, 2, 1)),
        expected: &[
            (1998, 1, Days::Range(1, 31)),
            (1999, 1, Days::Range(1, 31)),
            (2000, 1, Days::Range(1, 31)),
        ],
    },
    DayCase {
        name: "Every day in January, for 3 years (the DAILY spelling)",
        dtstart: (1998, 1, 1),
        rule: "FREQ=DAILY;UNTIL=20000131T140000Z;BYMONTH=1",
        exdates: &[],
        window: ((1998, 1, 1), (2000, 2, 1)),
        expected: &[
            (1998, 1, Days::Range(1, 31)),
            (1999, 1, Days::Range(1, 31)),
            (2000, 1, Days::Range(1, 31)),
        ],
    },
    DayCase {
        name: "Weekly for 10 occurrences",
        dtstart: (1997, 9, 2),
        rule: "FREQ=WEEKLY;COUNT=10",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 12, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 9, 16, 23, 30])),
            (1997, 10, Days::List(&[7, 14, 21, 28])),
            (1997, 11, Days::List(&[4])),
        ],
    },
    DayCase {
        name: "Weekly until December 24, 1997",
        dtstart: (1997, 9, 2),
        rule: "FREQ=WEEKLY;UNTIL=19971224T000000Z",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 1, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 9, 16, 23, 30])),
            (1997, 10, Days::List(&[7, 14, 21, 28])),
            (1997, 11, Days::List(&[4, 11, 18, 25])),
            (1997, 12, Days::List(&[2, 9, 16, 23])),
        ],
    },
    DayCase {
        name: "Every other week - forever",
        dtstart: (1997, 9, 2),
        rule: "FREQ=WEEKLY;INTERVAL=2;WKST=SU",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 3, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 16, 30])),
            (1997, 10, Days::List(&[14, 28])),
            (1997, 11, Days::List(&[11, 25])),
            (1997, 12, Days::List(&[9, 23])),
            (1998, 1, Days::List(&[6, 20])),
            (1998, 2, Days::List(&[3, 17])),
        ],
    },
    DayCase {
        name: "Weekly on Tuesday and Thursday for five weeks (the UNTIL spelling)",
        dtstart: (1997, 9, 2),
        rule: "FREQ=WEEKLY;UNTIL=19971007T000000Z;WKST=SU;BYDAY=TU,TH",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 11, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 4, 9, 11, 16, 18, 23, 25, 30])),
            (1997, 10, Days::List(&[2])),
        ],
    },
    DayCase {
        name: "Weekly on Tuesday and Thursday for five weeks (the COUNT spelling)",
        dtstart: (1997, 9, 2),
        rule: "FREQ=WEEKLY;COUNT=10;WKST=SU;BYDAY=TU,TH",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 11, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 4, 9, 11, 16, 18, 23, 25, 30])),
            (1997, 10, Days::List(&[2])),
        ],
    },
    DayCase {
        name: "Every other week on Monday, Wednesday, and Friday until December 24, 1997",
        dtstart: (1997, 9, 1),
        rule: "FREQ=WEEKLY;INTERVAL=2;UNTIL=19971224T000000Z;WKST=SU;BYDAY=MO,WE,FR",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 1, 1)),
        expected: &[
            (1997, 9, Days::List(&[1, 3, 5, 15, 17, 19, 29])),
            (1997, 10, Days::List(&[1, 3, 13, 15, 17, 27, 29, 31])),
            (1997, 11, Days::List(&[10, 12, 14, 24, 26, 28])),
            (1997, 12, Days::List(&[8, 10, 12, 22])),
        ],
    },
    DayCase {
        name: "Every other week on Tuesday and Thursday, for 8 occurrences",
        dtstart: (1997, 9, 2),
        rule: "FREQ=WEEKLY;INTERVAL=2;COUNT=8;WKST=SU;BYDAY=TU,TH",
        exdates: &[],
        window: ((1997, 9, 1), (1997, 11, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 4, 16, 18, 30])),
            (1997, 10, Days::List(&[2, 14, 16])),
        ],
    },
    DayCase {
        name: "Monthly on the first Friday for 10 occurrences",
        dtstart: (1997, 9, 5),
        rule: "FREQ=MONTHLY;COUNT=10;BYDAY=1FR",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 7, 1)),
        expected: &[
            (1997, 9, Days::List(&[5])),
            (1997, 10, Days::List(&[3])),
            (1997, 11, Days::List(&[7])),
            (1997, 12, Days::List(&[5])),
            (1998, 1, Days::List(&[2])),
            (1998, 2, Days::List(&[6])),
            (1998, 3, Days::List(&[6])),
            (1998, 4, Days::List(&[3])),
            (1998, 5, Days::List(&[1])),
            (1998, 6, Days::List(&[5])),
        ],
    },
    DayCase {
        name: "Monthly on the first Friday until December 24, 1997",
        dtstart: (1997, 9, 5),
        rule: "FREQ=MONTHLY;UNTIL=19971224T000000Z;BYDAY=1FR",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 1, 1)),
        expected: &[
            (1997, 9, Days::List(&[5])),
            (1997, 10, Days::List(&[3])),
            (1997, 11, Days::List(&[7])),
            (1997, 12, Days::List(&[5])),
        ],
    },
    DayCase {
        name: "Every other month on the first and last Sunday of the month for 10 occurrences",
        dtstart: (1997, 9, 7),
        rule: "FREQ=MONTHLY;INTERVAL=2;COUNT=10;BYDAY=1SU,-1SU",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 7, 1)),
        expected: &[
            (1997, 9, Days::List(&[7, 28])),
            (1997, 11, Days::List(&[2, 30])),
            (1998, 1, Days::List(&[4, 25])),
            (1998, 3, Days::List(&[1, 29])),
            (1998, 5, Days::List(&[3, 31])),
        ],
    },
    DayCase {
        name: "Monthly on the second-to-last Monday of the month for 6 months",
        dtstart: (1997, 9, 22),
        rule: "FREQ=MONTHLY;COUNT=6;BYDAY=-2MO",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 3, 1)),
        expected: &[
            (1997, 9, Days::List(&[22])),
            (1997, 10, Days::List(&[20])),
            (1997, 11, Days::List(&[17])),
            (1997, 12, Days::List(&[22])),
            (1998, 1, Days::List(&[19])),
            (1998, 2, Days::List(&[16])),
        ],
    },
    DayCase {
        name: "Monthly on the third-to-the-last day of the month, forever",
        dtstart: (1997, 9, 28),
        rule: "FREQ=MONTHLY;BYMONTHDAY=-3",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 3, 1)),
        expected: &[
            (1997, 9, Days::List(&[28])),
            (1997, 10, Days::List(&[29])),
            (1997, 11, Days::List(&[28])),
            (1997, 12, Days::List(&[29])),
            (1998, 1, Days::List(&[29])),
            (1998, 2, Days::List(&[26])),
        ],
    },
    DayCase {
        name: "Monthly on the 2nd and 15th of the month for 10 occurrences",
        dtstart: (1997, 9, 2),
        rule: "FREQ=MONTHLY;COUNT=10;BYMONTHDAY=2,15",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 3, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 15])),
            (1997, 10, Days::List(&[2, 15])),
            (1997, 11, Days::List(&[2, 15])),
            (1997, 12, Days::List(&[2, 15])),
            (1998, 1, Days::List(&[2, 15])),
        ],
    },
    DayCase {
        name: "Monthly on the first and last day of the month for 10 occurrences",
        dtstart: (1997, 9, 30),
        rule: "FREQ=MONTHLY;COUNT=10;BYMONTHDAY=1,-1",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 3, 1)),
        expected: &[
            (1997, 9, Days::List(&[30])),
            (1997, 10, Days::List(&[1, 31])),
            (1997, 11, Days::List(&[1, 30])),
            (1997, 12, Days::List(&[1, 31])),
            (1998, 1, Days::List(&[1, 31])),
            (1998, 2, Days::List(&[1])),
        ],
    },
    DayCase {
        name: "Every 18 months on the 10th thru 15th of the month for 10 occurrences",
        dtstart: (1997, 9, 10),
        rule: "FREQ=MONTHLY;INTERVAL=18;COUNT=10;BYMONTHDAY=10,11,12,13,14,15",
        exdates: &[],
        window: ((1997, 9, 1), (1999, 5, 1)),
        expected: &[
            (1997, 9, Days::Range(10, 15)),
            (1999, 3, Days::Range(10, 13)),
        ],
    },
    DayCase {
        name: "Every Tuesday, every other month",
        dtstart: (1997, 9, 2),
        rule: "FREQ=MONTHLY;INTERVAL=2;BYDAY=TU",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 4, 1)),
        expected: &[
            (1997, 9, Days::List(&[2, 9, 16, 23, 30])),
            (1997, 11, Days::List(&[4, 11, 18, 25])),
            (1998, 1, Days::List(&[6, 13, 20, 27])),
            (1998, 3, Days::List(&[3, 10, 17, 24, 31])),
        ],
    },
    DayCase {
        name: "Yearly in June and July for 10 occurrences",
        dtstart: (1997, 6, 10),
        rule: "FREQ=YEARLY;COUNT=10;BYMONTH=6,7",
        exdates: &[],
        window: ((1997, 1, 1), (2002, 1, 1)),
        expected: &[
            (1997, 6, Days::List(&[10])),
            (1997, 7, Days::List(&[10])),
            (1998, 6, Days::List(&[10])),
            (1998, 7, Days::List(&[10])),
            (1999, 6, Days::List(&[10])),
            (1999, 7, Days::List(&[10])),
            (2000, 6, Days::List(&[10])),
            (2000, 7, Days::List(&[10])),
            (2001, 6, Days::List(&[10])),
            (2001, 7, Days::List(&[10])),
        ],
    },
    DayCase {
        name: "Every other year on January, February, and March for 10 occurrences",
        dtstart: (1997, 3, 10),
        rule: "FREQ=YEARLY;INTERVAL=2;COUNT=10;BYMONTH=1,2,3",
        exdates: &[],
        window: ((1997, 1, 1), (2004, 1, 1)),
        expected: &[
            (1997, 3, Days::List(&[10])),
            (1999, 1, Days::List(&[10])),
            (1999, 2, Days::List(&[10])),
            (1999, 3, Days::List(&[10])),
            (2001, 1, Days::List(&[10])),
            (2001, 2, Days::List(&[10])),
            (2001, 3, Days::List(&[10])),
            (2003, 1, Days::List(&[10])),
            (2003, 2, Days::List(&[10])),
            (2003, 3, Days::List(&[10])),
        ],
    },
    DayCase {
        name: "Every third year on the 1st, 100th, and 200th day for 10 occurrences",
        dtstart: (1997, 1, 1),
        rule: "FREQ=YEARLY;INTERVAL=3;COUNT=10;BYYEARDAY=1,100,200",
        exdates: &[],
        window: ((1997, 1, 1), (2007, 1, 1)),
        expected: &[
            (1997, 1, Days::List(&[1])),
            (1997, 4, Days::List(&[10])),
            (1997, 7, Days::List(&[19])),
            (2000, 1, Days::List(&[1])),
            (2000, 4, Days::List(&[9])),
            (2000, 7, Days::List(&[18])),
            (2003, 1, Days::List(&[1])),
            (2003, 4, Days::List(&[10])),
            (2003, 7, Days::List(&[19])),
            (2006, 1, Days::List(&[1])),
        ],
    },
    DayCase {
        name: "Every 20th Monday of the year, forever",
        dtstart: (1997, 5, 19),
        rule: "FREQ=YEARLY;BYDAY=20MO",
        exdates: &[],
        window: ((1997, 1, 1), (2000, 1, 1)),
        expected: &[
            (1997, 5, Days::List(&[19])),
            (1998, 5, Days::List(&[18])),
            (1999, 5, Days::List(&[17])),
        ],
    },
    DayCase {
        name: "Monday of week number 20 (the default start of the week being Monday), forever",
        dtstart: (1997, 5, 12),
        rule: "FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO",
        exdates: &[],
        window: ((1997, 1, 1), (2000, 1, 1)),
        expected: &[
            (1997, 5, Days::List(&[12])),
            (1998, 5, Days::List(&[11])),
            (1999, 5, Days::List(&[17])),
        ],
    },
    DayCase {
        name: "Every Thursday in March, forever",
        dtstart: (1997, 3, 13),
        rule: "FREQ=YEARLY;BYMONTH=3;BYDAY=TH",
        exdates: &[],
        window: ((1997, 1, 1), (2000, 1, 1)),
        expected: &[
            (1997, 3, Days::List(&[13, 20, 27])),
            (1998, 3, Days::List(&[5, 12, 19, 26])),
            (1999, 3, Days::List(&[4, 11, 18, 25])),
        ],
    },
    DayCase {
        name: "Every Thursday, but only during June, July, and August, forever",
        dtstart: (1997, 6, 5),
        rule: "FREQ=YEARLY;BYDAY=TH;BYMONTH=6,7,8",
        exdates: &[],
        window: ((1997, 1, 1), (2000, 1, 1)),
        expected: &[
            (1997, 6, Days::List(&[5, 12, 19, 26])),
            (1997, 7, Days::List(&[3, 10, 17, 24, 31])),
            (1997, 8, Days::List(&[7, 14, 21, 28])),
            (1998, 6, Days::List(&[4, 11, 18, 25])),
            (1998, 7, Days::List(&[2, 9, 16, 23, 30])),
            (1998, 8, Days::List(&[6, 13, 20, 27])),
            (1999, 6, Days::List(&[3, 10, 17, 24])),
            (1999, 7, Days::List(&[1, 8, 15, 22, 29])),
            (1999, 8, Days::List(&[5, 12, 19, 26])),
        ],
    },
    DayCase {
        name: "Every Friday the 13th, forever",
        dtstart: (1997, 9, 2),
        rule: "FREQ=MONTHLY;BYDAY=FR;BYMONTHDAY=13",
        exdates: &[(1997, 9, 2)],
        window: ((1997, 9, 1), (2001, 1, 1)),
        expected: &[
            (1998, 2, Days::List(&[13])),
            (1998, 3, Days::List(&[13])),
            (1998, 11, Days::List(&[13])),
            (1999, 8, Days::List(&[13])),
            (2000, 10, Days::List(&[13])),
        ],
    },
    DayCase {
        name: "The first Saturday that follows the first Sunday of the month, forever",
        dtstart: (1997, 9, 13),
        rule: "FREQ=MONTHLY;BYDAY=SA;BYMONTHDAY=7,8,9,10,11,12,13",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 7, 1)),
        expected: &[
            (1997, 9, Days::List(&[13])),
            (1997, 10, Days::List(&[11])),
            (1997, 11, Days::List(&[8])),
            (1997, 12, Days::List(&[13])),
            (1998, 1, Days::List(&[10])),
            (1998, 2, Days::List(&[7])),
            (1998, 3, Days::List(&[7])),
            (1998, 4, Days::List(&[11])),
            (1998, 5, Days::List(&[9])),
            (1998, 6, Days::List(&[13])),
        ],
    },
    DayCase {
        name: "Every 4 years, the first Tuesday after a Monday in November, forever",
        dtstart: (1996, 11, 5),
        rule: "FREQ=YEARLY;INTERVAL=4;BYMONTH=11;BYDAY=TU;BYMONTHDAY=2,3,4,5,6,7,8",
        exdates: &[],
        window: ((1996, 1, 1), (2005, 1, 1)),
        expected: &[
            (1996, 11, Days::List(&[5])),
            (2000, 11, Days::List(&[7])),
            (2004, 11, Days::List(&[2])),
        ],
    },
    DayCase {
        name: "The third instance into the month of one of Tuesday, Wednesday, or Thursday, \
               for the next 3 months",
        dtstart: (1997, 9, 4),
        rule: "FREQ=MONTHLY;COUNT=3;BYDAY=TU,WE,TH;BYSETPOS=3",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 1, 1)),
        expected: &[
            (1997, 9, Days::List(&[4])),
            (1997, 10, Days::List(&[7])),
            (1997, 11, Days::List(&[6])),
        ],
    },
    DayCase {
        name: "The second-to-last weekday of the month",
        dtstart: (1997, 9, 29),
        rule: "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-2",
        exdates: &[],
        window: ((1997, 9, 1), (1998, 4, 1)),
        expected: &[
            (1997, 9, Days::List(&[29])),
            (1997, 10, Days::List(&[30])),
            (1997, 11, Days::List(&[27])),
            (1997, 12, Days::List(&[30])),
            (1998, 1, Days::List(&[29])),
            (1998, 2, Days::List(&[26])),
            (1998, 3, Days::List(&[30])),
        ],
    },
    DayCase {
        name: "An example where the days generated makes a difference because of WKST (WKST=MO)",
        dtstart: (1997, 8, 5),
        rule: "FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=MO",
        exdates: &[],
        window: ((1997, 8, 1), (1997, 9, 1)),
        expected: &[(1997, 8, Days::List(&[5, 10, 19, 24]))],
    },
    DayCase {
        name: "changing only WKST from MO to SU, yields different results",
        dtstart: (1997, 8, 5),
        rule: "FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=SU",
        exdates: &[],
        window: ((1997, 8, 1), (1997, 9, 1)),
        expected: &[(1997, 8, Days::List(&[5, 17, 19, 31]))],
    },
    DayCase {
        name: "An example where an invalid date (i.e., February 30) is ignored",
        dtstart: (2007, 1, 15),
        rule: "FREQ=MONTHLY;BYMONTHDAY=15,30;COUNT=5",
        exdates: &[],
        window: ((2007, 1, 1), (2007, 5, 1)),
        expected: &[
            (2007, 1, Days::List(&[15, 30])),
            (2007, 2, Days::List(&[15])),
            (2007, 3, Days::List(&[15, 30])),
        ],
    },
];

/// Section 3.8.5.3's examples that name clock readings inside a day.
const CLOCK_CASES: &[ClockCase] = &[
    ClockCase {
        name: "Every 3 hours from 9:00 AM to 5:00 PM on a specific day",
        dtstart: (1997, 9, 2, 9, 0),
        rule: "FREQ=HOURLY;INTERVAL=3;UNTIL=19970902T170000Z",
        window: ((1997, 9, 1), (1997, 9, 4)),
        days: &[(1997, 9, 2)],
        times: &[(9, 0), (12, 0), (15, 0)],
    },
    ClockCase {
        name: "Every 15 minutes for 6 occurrences",
        dtstart: (1997, 9, 2, 9, 0),
        rule: "FREQ=MINUTELY;INTERVAL=15;COUNT=6",
        window: ((1997, 9, 1), (1997, 9, 4)),
        days: &[(1997, 9, 2)],
        times: &[(9, 0), (9, 15), (9, 30), (9, 45), (10, 0), (10, 15)],
    },
    ClockCase {
        name: "Every hour and a half for 4 occurrences",
        dtstart: (1997, 9, 2, 9, 0),
        rule: "FREQ=MINUTELY;INTERVAL=90;COUNT=4",
        window: ((1997, 9, 1), (1997, 9, 4)),
        days: &[(1997, 9, 2)],
        times: &[(9, 0), (10, 30), (12, 0), (13, 30)],
    },
    ClockCase {
        name: "Every 20 minutes from 9:00 AM to 4:40 PM every day (the DAILY spelling)",
        dtstart: (1997, 9, 2, 9, 0),
        rule: "FREQ=DAILY;BYHOUR=9,10,11,12,13,14,15,16;BYMINUTE=0,20,40",
        window: ((1997, 9, 2), (1997, 9, 4)),
        days: &[(1997, 9, 2), (1997, 9, 3)],
        times: EVERY_TWENTY,
    },
    ClockCase {
        name: "Every 20 minutes from 9:00 AM to 4:40 PM every day (the MINUTELY spelling)",
        dtstart: (1997, 9, 2, 9, 0),
        rule: "FREQ=MINUTELY;INTERVAL=20;BYHOUR=9,10,11,12,13,14,15,16",
        window: ((1997, 9, 2), (1997, 9, 4)),
        days: &[(1997, 9, 2), (1997, 9, 3)],
        times: EVERY_TWENTY,
    },
];

/// The instant `year-month-day hour:minute:00` on the timeline the caller resolved.
///
/// Fallible rather than total, so a mistyped literal in a table is a named failure in the test
/// that read it rather than a plausible wrong instant compared against another one.
fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Option<Instant> {
    let date = CivilDate::from_ymd(year, month, day)?;
    let time = CivilTime::from_hms(hour, minute, 0)?;
    CivilDateTime::new(date, time).at_offset(UtcOffset::UTC)
}

/// Midnight on `date`, which is how both edges of every window in the tables are written.
fn midnight(date: (u16, u8, u8)) -> Option<Instant> {
    at(date.0, date.1, date.2, 0, 0)
}

/// The half-open window a case asks about.
fn window_of(edges: ((u16, u8, u8), (u16, u8, u8))) -> Option<Window> {
    Window::new(midnight(edges.0)?, midnight(edges.1)?)
}

/// The answer a day case states, flattened onto the timeline in the RFC's own order.
fn day_answer(groups: &[(u16, u8, Days)]) -> Option<Vec<Instant>> {
    let mut found = Vec::new();
    for (year, month, days) in groups {
        for day in days.days() {
            found.push(at(*year, *month, day, NINE_AM.0, NINE_AM.1)?);
        }
    }
    Some(found)
}

/// The answer a clock case states: every listed reading of every listed day, in order.
fn clock_answer(days: &[(u16, u8, u8)], times: &[(u8, u8)]) -> Option<Vec<Instant>> {
    let mut found = Vec::new();
    for (year, month, day) in days {
        for (hour, minute) in times {
            found.push(at(*year, *month, *day, *hour, *minute)?);
        }
    }
    Some(found)
}

/// What one search produced, and everything it said on the way.
#[derive(Debug)]
struct Run {
    /// The effective start of every occurrence, in the order it was emitted.
    starts: Vec<Instant>,
    /// Whether the search ran to the end of the rule or the window rather than to the budget.
    complete: bool,
    /// What the search reported while it worked.
    reported: Vec<Diagnostic>,
}

/// Expand `rule` from `dtstart` over `window`, excluding `exclusions`.
///
/// One meter for the decode and the expansion together, which is the shape a caller reading a
/// file actually has: the same ledger paid for reading the value and pays for expanding it.
fn expand(dtstart: Instant, rule: &str, exclusions: &[Instant], window: Window) -> Option<Run> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let decoded = parse_recur(rule.as_bytes(), &mut meter, &mut reported).ok()?;
    let input = RecurrenceInput::new(
        dtstart,
        ValueKind::DateTime,
        Some(&decoded),
        &[],
        exclusions,
        OverrideSet::empty(),
        &mut meter,
    )
    .ok()?;

    let mut starts = Vec::new();
    let complete = {
        let mut search = input.search(window, &mut meter, &mut reported);
        for step in search.by_ref() {
            match step.occurrence() {
                Some(occurrence) => starts.push(occurrence.start()),
                None => break,
            }
        }
        search.outcome().is_complete()
    };
    Some(Run {
        starts,
        complete,
        reported,
    })
}

/// Every worked example naming whole days answers exactly what the RFC prints beside it.
#[test]
fn every_day_example_of_section_3_8_5_3_answers_what_the_rfc_prints() {
    assert_eq!(
        DAY_CASES.len(),
        37,
        "the table holds every day example the RFC prints"
    );
    for case in DAY_CASES {
        let (year, month, day) = case.dtstart;
        let dtstart = at(year, month, day, NINE_AM.0, NINE_AM.1).expect(case.name);
        let window = window_of(case.window).expect(case.name);
        let exclusions: Vec<Instant> = case
            .exdates
            .iter()
            .map(|date| at(date.0, date.1, date.2, NINE_AM.0, NINE_AM.1).expect(case.name))
            .collect();
        let expected = day_answer(case.expected).expect(case.name);

        let run = expand(dtstart, case.rule, &exclusions, window).expect(case.name);
        assert_eq!(run.starts, expected, "{}", case.name);
        assert!(run.complete, "{}", case.name);
        assert!(
            run.reported
                .iter()
                .all(|entry| entry.severity() == Severity::Note),
            "{} reported something worse than a note",
            case.name
        );
    }
}

/// Every worked example naming clock readings answers exactly what the RFC prints beside it.
#[test]
fn every_clock_example_of_section_3_8_5_3_answers_what_the_rfc_prints() {
    assert_eq!(
        CLOCK_CASES.len(),
        5,
        "the table holds every intraday example the RFC prints"
    );
    for case in CLOCK_CASES {
        let (year, month, day, hour, minute) = case.dtstart;
        let dtstart = at(year, month, day, hour, minute).expect(case.name);
        let window = window_of(case.window).expect(case.name);
        let expected = clock_answer(case.days, case.times).expect(case.name);

        let run = expand(dtstart, case.rule, &[], window).expect(case.name);
        assert_eq!(run.starts, expected, "{}", case.name);
        assert!(run.complete, "{}", case.name);
    }
}

/// The invalid-date example reports the instance it declined to invent, and does not clamp it.
///
/// RFC 5545 section 3.3.10 requires February 30 to be ignored rather than moved to a nearby
/// date. Ignoring it silently and ignoring it audibly are both conforming; this workspace's
/// `docs/adr/0009` chooses audibly, and the code is the one the golden list carries.
#[test]
fn the_invalid_date_example_says_what_it_declined_to_invent() {
    let dtstart = at(2007, 1, 15, 9, 0).expect("the RFC's own DTSTART");
    let window = window_of(((2007, 1, 1), (2007, 5, 1))).expect("the window around the answer");
    let run = expand(
        dtstart,
        "FREQ=MONTHLY;BYMONTHDAY=15,30;COUNT=5",
        &[],
        window,
    )
    .expect("the RFC's own rule");

    assert!(
        run.reported.iter().any(|entry| {
            entry.code() == DiagnosticCode::NonexistentRecurrenceInstance
                && entry.severity() == Severity::Note
        }),
        "February 30 is reported as the instance it is, not clamped to February 28"
    );
    let clamped = at(2007, 2, 28, 9, 0).expect("the date a clamping engine would invent");
    assert!(!run.starts.contains(&clamped));
}

/// The terminal step is a `SearchStep` and not something `flatten` can drop.
///
/// Not an RFC example: it is the property `docs/adr/0002` asks the corpus to hold from outside
/// the crate, and outside is here. A caller that walks the steps sees the same ten days as the
/// caller that reads them through [`SearchStep::occurrence`].
#[test]
fn a_caller_outside_the_crate_cannot_flatten_the_terminal_step_away() {
    let dtstart = at(1997, 9, 2, 9, 0).expect("the RFC's own DTSTART");
    let window = window_of(((1997, 9, 1), (1997, 10, 1))).expect("the window around the answer");
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let rule =
        parse_recur(b"FREQ=DAILY;COUNT=10", &mut meter, &mut reported).expect("the RFC's own rule");
    let input = RecurrenceInput::new(
        dtstart,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        icalkit_conformance::internal::recur::OverrideSet::empty(),
        &mut meter,
    )
    .expect("a series with no lists to check");

    let steps: Vec<SearchStep<'_>> = input.search(window, &mut meter, &mut reported).collect();
    assert_eq!(steps.len(), 10);
    assert!(steps.iter().all(|step| !step.is_terminal()));
}
