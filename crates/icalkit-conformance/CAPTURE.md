# Producer capture intake

This is the maintainer procedure for the external evidence that cannot be manufactured by the
workspace. It covers Google Calendar Web and Microsoft 365 Outlook Web first. Apple Calendar uses
the same scenario when that capture becomes available.

The raw ICS is evidence and may contain account identifiers even when the calendar itself is
disposable. Keep the bundle outside the repository. Do not open the ICS in an editor, resave it,
paste it through a text field, or commit it. The rendering image must be cropped to the calendar
grid before intake so it contains no account name, email address, avatar, notification, or other
unrelated UI.

## Scenario dst-gap-daily-series-v1

Create a new empty calendar in each producer. Set the calendar or event time zone to
America/New_York, then create this appointment natively in that producer:

- title: ICALKIT GAP TEST
- first start: 2027-03-13 02:30
- first end: 2027-03-13 02:45
- recurrence: daily, ending 2027-03-15 or after three occurrences
- attendees, location, description, conferencing, and alarms: absent

2027-03-14 02:30 does not exist in that zone. Show March 13 through March 15 in the producer UI and
record the March 14 result in one bucket:

| outcome | rendered March 14 local time |
| --- | --- |
| skipped | absent; rendered_local is null |
| offset-before | 03:30 |
| gap-end | 03:00 |
| offset-after | 01:30 |
| other | the observed local time, plus a non-empty notes field |

The producer UI is the observation. Expanding the exported RRULE with icalkit is not a substitute.

## Export and bundle

For Google Calendar Web, export only the disposable calendar from My calendars, Settings and
sharing, Export calendar. Google documents that single-calendar flow at
https://support.google.com/calendar/answer/37111?hl=en.

For Outlook Web, publish only the disposable calendar from Settings, Calendar, Shared calendars,
download the generated ICS link, and immediately select Unpublish. Microsoft documents both
publication and removal at
https://support.microsoft.com/en-us/outlook/sharing/share-an-outlook-calendar-as-view-only-with-others.
Never include the published URL in the bundle.

Place three sibling files in a directory outside the workspace:

    capture.json
    export.ics
    render.png

Copy corpus/capture.v1.example.json to capture.json and change its metadata without changing the
schema. Microsoft 365 uses client "Microsoft 365" and surface "Outlook Web". account_type is
"consumer" or "organization". If the web producer exposes no stable build identifier, version
"web" is honest; the observation date, source hash, and the export's own PRODID remain in the
record.

Audit the bundle without copying it:

    cargo run -p icalkit-conformance --bin capture-audit -- C:\outside-repo\capture\capture.json

The command accepts one versioned bundle, verifies the exact test series and rendering bucket,
checks the image signature, computes SHA-256 for both artifacts, and prints one JSON report. It
rejects a bundle located anywhere inside the workspace and never writes or prints raw bytes or
paths. A successful status is ready-for-reduction, not privacy-approved: a maintainer must still
inspect, anonymize, and minimize the ICS before committing a fixture.

## Corpus admission

The reduced fixture preserves the producer behavior but contains no personal data. Its captured
manifest row records the client, surface, account type, version, observation date, reduction,
raw source SHA-256, rendering bucket, rendering path, and rendering SHA-256. The cropped rendering
may be committed only after a second privacy review.

A Google or Microsoft capture is a partial observation. ADR-0011 requires all three named
producers before the gap default can move, and a skip by any producer prevents the flip. These
captures therefore do not change the default or authorize a CommonClientsV1 repair until the
complete threshold and the separate repair evidence are present.
