// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The closed element vocabulary, keyed on a resolved namespace and a local name.
//!
//! A prefix is a per-document choice its author made and nothing else. `DAV:` may be bound to
//! `D:`, to `d:`, to no prefix at all through a default declaration, or to a different prefix
//! on every element of one body, and the servers people actually run disagree about which.
//! `<d:multistatus xmlns:d="DAV:">` and `<multistatus xmlns="DAV:">` are the same element;
//! `<D:multistatus xmlns:D="http://evil.example/not-dav">` is a different one and must never
//! be read as the first. A table keyed on the literal string `D:multistatus` gets all three
//! wrong, silently, against the most widely deployed CalDAV software there is.
//!
//! So nothing here matches a prefix. [`Namespace::from_uri`] classifies a *resolved* URI,
//! [`ElementName::resolve`] takes that classification and a local name, and no public
//! function in this crate accepts a prefix at all. The prefixes this crate writes — `D:`,
//! `C:`, `CS:` — are an output choice, declared on the root element of everything it emits,
//! and never an input assumption.
//!
//! The table is unconditional. A row exists for every element of RFC 4918, RFC 4791, RFC 6578
//! and RFC 6638 this crate names, whatever features are compiled, so a build that cannot
//! honor a `REPORT` answers [`DavError::Unsupported`](crate::DavError::Unsupported) instead of
//! skipping the request as though the server had made it up.

/// A resolved namespace URI, classified.
///
/// `Other` keeps the URI's octets rather than discarding them, because a foreign element is
/// reported to the caller's sink and "some namespace" is not something a caller can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Namespace<'a> {
    /// `DAV:`, RFC 4918. Not a URL; the trailing colon is part of it.
    Dav,
    /// `urn:ietf:params:xml:ns:caldav`, RFC 4791.
    CalDav,
    /// `http://calendarserver.org/ns/`, the vendor namespace `getctag` lives in.
    ///
    /// Not an IETF namespace and modeled anyway, because the property every widely deployed
    /// client polls a collection with lives in it. Recognizing it is what keeps a real
    /// server's `PROPFIND` response readable rather than two thirds skipped.
    CalendarServer,
    /// Anything else, with the URI kept.
    Other(&'a [u8]),
}

impl<'a> Namespace<'a> {
    /// The URI of `DAV:`.
    pub const DAV_URI: &'static [u8] = b"DAV:";
    /// The URI of the CalDAV namespace.
    pub const CALDAV_URI: &'static [u8] = b"urn:ietf:params:xml:ns:caldav";
    /// The URI of the `CalendarServer` vendor namespace.
    pub const CALENDARSERVER_URI: &'static [u8] = b"http://calendarserver.org/ns/";

    /// Classify a resolved namespace URI.
    ///
    /// Compared octet for octet. A namespace URI is not a URL to be normalized: XML
    /// Namespaces 1.0 section 2.3 makes identity string comparison, so `DAV:` and `dav:` are
    /// different namespaces and this function says so.
    #[must_use]
    pub const fn from_uri(uri: &'a [u8]) -> Self {
        if equal(uri, Self::DAV_URI) {
            Self::Dav
        } else if equal(uri, Self::CALDAV_URI) {
            Self::CalDav
        } else if equal(uri, Self::CALENDARSERVER_URI) {
            Self::CalendarServer
        } else {
            Self::Other(uri)
        }
    }

    /// The URI this namespace resolves to.
    #[must_use]
    pub const fn uri(self) -> &'a [u8] {
        match self {
            Self::Dav => Self::DAV_URI,
            Self::CalDav => Self::CALDAV_URI,
            Self::CalendarServer => Self::CALENDARSERVER_URI,
            Self::Other(uri) => uri,
        }
    }

    /// Whether two namespaces are the same namespace.
    ///
    /// Written over the URIs rather than derived, so that a `Namespace<'static>` from the
    /// table and a `Namespace<'_>` borrowed from a body compare without either lifetime
    /// having to become the other.
    #[must_use]
    pub const fn is(self, other: Namespace<'_>) -> bool {
        equal(self.uri(), other.uri())
    }

    /// The prefix this crate writes for the namespace.
    ///
    /// An output choice and never an input assumption; see this module's own documentation.
    /// A foreign namespace has none, because this crate writes no element it has no row for.
    #[must_use]
    pub const fn write_prefix(self) -> &'static [u8] {
        match self {
            Self::Dav => b"D",
            Self::CalDav => b"C",
            Self::CalendarServer => b"CS",
            Self::Other(_) => b"",
        }
    }
}

/// Octet equality, `const` because the classification above is.
const fn equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut at = 0;
    while at < left.len() {
        if left[at] != right[at] {
            return false;
        }
        at = at.saturating_add(1);
    }
    true
}

/// A resolved element or attribute name: a namespace and a local name, never a prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QName<'a> {
    /// The namespace the name resolved into.
    pub namespace: Namespace<'a>,
    /// The part of the name after the colon, or the whole name when there was no prefix.
    pub local_name: &'a [u8],
}

impl<'a> QName<'a> {
    /// A name in a namespace.
    #[must_use]
    pub const fn new(namespace: Namespace<'a>, local_name: &'a [u8]) -> Self {
        Self {
            namespace,
            local_name,
        }
    }

    /// The row of the closed vocabulary this name resolves to, if there is one.
    #[must_use]
    pub fn known(self) -> Option<ElementName> {
        ElementName::resolve(self.namespace, self.local_name)
    }
}

/// A namespace and a local name, as the table records them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementSpec {
    /// The namespace the element belongs to.
    pub namespace: Namespace<'static>,
    /// The element's local name, exactly as its specification spells it.
    pub local_name: &'static str,
}

/// Every element of the closed vocabulary.
///
/// Rows whose local names collide across namespaces carry both facts in the variant name:
/// `DAV:prop` is [`ElementName::Prop`] and the `prop` inside a `calendar-data` request is
/// [`ElementName::CalendarDataProp`], because they are different elements that a table keyed
/// on local names alone would merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ElementName {
    /// `DAV:multistatus`, RFC 4918 section 14.16.
    Multistatus,
    /// `DAV:response`, RFC 4918 section 14.24.
    Response,
    /// `DAV:propstat`, RFC 4918 section 14.22.
    Propstat,
    /// `DAV:prop`, RFC 4918 section 14.18.
    Prop,
    /// `DAV:href`, RFC 4918 section 14.7.
    Href,
    /// `DAV:status`, RFC 4918 section 14.28.
    Status,
    /// `DAV:error`, RFC 4918 section 14.5.
    Error,
    /// `DAV:responsedescription`, RFC 4918 section 14.25.
    ResponseDescription,
    /// `DAV:location`, RFC 4918 section 14.9.
    Location,
    /// `DAV:propfind`, RFC 4918 section 14.20.
    Propfind,
    /// `DAV:allprop`, RFC 4918 section 14.2.
    Allprop,
    /// `DAV:propname`, RFC 4918 section 14.21.
    Propname,
    /// `DAV:include`, RFC 4918 section 14.8.
    Include,
    /// `DAV:getetag`, RFC 4918 section 15.6.
    Getetag,
    /// `DAV:getcontenttype`, RFC 4918 section 15.5.
    Getcontenttype,
    /// `DAV:getcontentlength`, RFC 4918 section 15.4.
    Getcontentlength,
    /// `DAV:getlastmodified`, RFC 4918 section 15.7.
    Getlastmodified,
    /// `DAV:displayname`, RFC 4918 section 15.2.
    Displayname,
    /// `DAV:resourcetype`, RFC 4918 section 15.9.
    Resourcetype,
    /// `DAV:collection`, RFC 4918 section 15.9.
    Collection,
    /// `DAV:principal`, RFC 3744 section 4.
    Principal,
    /// `DAV:owner`, RFC 3744 section 5.1.
    Owner,
    /// `DAV:current-user-principal`, RFC 5397 section 3.
    ///
    /// The one property that answers "who does this server think I am", which RFC 6638 makes
    /// the actor a scheduling decision is judged against.
    CurrentUserPrincipal,
    /// `DAV:principal-URL`, RFC 3744 section 4.2.
    PrincipalUrl,
    /// `DAV:supported-report-set`, RFC 3253 section 3.1.5.
    SupportedReportSet,
    /// `DAV:supported-report`, RFC 3253 section 3.1.5.
    SupportedReport,
    /// `DAV:report`, RFC 3253 section 3.1.5.
    Report,
    /// `DAV:sync-collection`, RFC 6578 section 3.
    SyncCollection,
    /// `DAV:sync-token`, RFC 6578 section 3.
    SyncToken,
    /// `DAV:sync-level`, RFC 6578 section 6.3.
    SyncLevel,
    /// `DAV:limit`, RFC 5323 section 5.17.
    Limit,
    /// `DAV:nresults`, RFC 5323 section 5.17.
    Nresults,
    /// `DAV:valid-sync-token`, RFC 6578 section 3.2, a precondition.
    ValidSyncToken,
    /// `DAV:number-of-matches-within-limits`, RFC 4918 section 11.
    NumberOfMatchesWithinLimits,
    /// `CALDAV:calendar-query`, RFC 4791 section 9.5.
    CalendarQuery,
    /// `CALDAV:calendar-multiget`, RFC 4791 section 9.10.
    CalendarMultiget,
    /// `CALDAV:free-busy-query`, RFC 4791 section 9.11.
    FreeBusyQuery,
    /// `CALDAV:calendar-data`, RFC 4791 section 9.6. The one element whose line endings are
    /// its content rather than its layout.
    CalendarData,
    /// `CALDAV:filter`, RFC 4791 section 9.7.
    Filter,
    /// `CALDAV:comp-filter`, RFC 4791 section 9.7.1.
    CompFilter,
    /// `CALDAV:prop-filter`, RFC 4791 section 9.7.2.
    PropFilter,
    /// `CALDAV:param-filter`, RFC 4791 section 9.7.3.
    ParamFilter,
    /// `CALDAV:is-not-defined`, RFC 4791 section 9.7.4.
    IsNotDefined,
    /// `CALDAV:text-match`, RFC 4791 section 9.7.5.
    TextMatch,
    /// `CALDAV:time-range`, RFC 4791 section 9.9.
    TimeRange,
    /// `CALDAV:comp` inside a `calendar-data` request, RFC 4791 section 9.6.1.
    CalendarDataComp,
    /// `CALDAV:allcomp` inside a `calendar-data` request, RFC 4791 section 9.6.2.
    CalendarDataAllcomp,
    /// `CALDAV:allprop` inside a `calendar-data` request, RFC 4791 section 9.6.4.
    CalendarDataAllprop,
    /// `CALDAV:prop` inside a `calendar-data` request, RFC 4791 section 9.6.5.
    CalendarDataProp,
    /// `CALDAV:expand`, RFC 4791 section 9.6.5.
    Expand,
    /// `CALDAV:limit-recurrence-set`, RFC 4791 section 9.6.6.
    LimitRecurrenceSet,
    /// `CALDAV:limit-freebusy-set`, RFC 4791 section 9.6.7.
    LimitFreebusySet,
    /// `CALDAV:calendar`, RFC 4791 section 4.2, a resource type.
    Calendar,
    /// `CALDAV:calendar-home-set`, RFC 4791 section 6.2.1.
    CalendarHomeSet,
    /// `CALDAV:calendar-description`, RFC 4791 section 5.2.1.
    CalendarDescription,
    /// `CALDAV:calendar-timezone`, RFC 4791 section 5.2.2.
    CalendarTimezone,
    /// `CALDAV:supported-calendar-component-set`, RFC 4791 section 5.2.3.
    SupportedCalendarComponentSet,
    /// `CALDAV:supported-calendar-data`, RFC 4791 section 5.2.4.
    SupportedCalendarData,
    /// `CALDAV:max-resource-size`, RFC 4791 section 5.2.5.
    MaxResourceSize,
    /// `CALDAV:min-date-time`, RFC 4791 section 5.2.6.
    MinDateTime,
    /// `CALDAV:max-date-time`, RFC 4791 section 5.2.7.
    MaxDateTime,
    /// `CALDAV:max-instances`, RFC 4791 section 5.2.8.
    MaxInstances,
    /// `CALDAV:max-attendees-per-instance`, RFC 4791 section 5.2.9.
    MaxAttendeesPerInstance,
    /// `CALDAV:supported-filter`, RFC 4791 section 9.7, a precondition.
    SupportedFilter,
    /// `CALDAV:valid-calendar-data`, RFC 4791 section 5.3.2.1, a precondition.
    ValidCalendarData,
    /// `CALDAV:valid-calendar-object-resource`, RFC 4791 section 5.3.2.1, a precondition.
    ValidCalendarObjectResource,
    /// `CALDAV:no-uid-conflict`, RFC 4791 section 5.3.2.1, a precondition.
    NoUidConflict,
    /// `CALDAV:calendar-user-address-set`, RFC 6638 section 2.4.1.
    ///
    /// The mapping from the principal the server authenticated to the `CAL-ADDRESS` an
    /// `ORGANIZER` or `ATTENDEE` line is written with. Without it a principal `href` and a
    /// `mailto:` are two identifiers nothing joins.
    CalendarUserAddressSet,
    /// `CALDAV:schedule-inbox-URL`, RFC 6638 section 2.2.
    ScheduleInboxUrl,
    /// `CALDAV:schedule-outbox-URL`, RFC 6638 section 2.1.
    ScheduleOutboxUrl,
    /// `CALDAV:schedule-tag`, RFC 6638 section 3.2.10.
    ///
    /// The `ETag`'s sibling for scheduling: it changes only when the scheduling-relevant part
    /// of an object changes, so an attendee's reply does not invalidate the organizer's copy.
    ScheduleTag,
    /// `CALDAV:schedule-calendar-transp`, RFC 6638 section 9.1.
    ScheduleCalendarTransp,
    /// `CALDAV:allowed-organizer-scheduling-object-change`, RFC 6638 section 3.2.1.
    ///
    /// The refusal a server writes when a stored copy's `ORGANIZER` moved: the one defense a
    /// file-level scheduling gate cannot supply, because it needs the copy that was there
    /// before the write.
    AllowedOrganizerSchedulingObjectChange,
    /// `CALDAV:allowed-attendee-scheduling-object-change`, RFC 6638 section 3.2.1.
    AllowedAttendeeSchedulingObjectChange,
    /// `CALDAV:unique-scheduling-object-resource`, RFC 6638 section 3.2.1.
    UniqueSchedulingObjectResource,
    /// `CALDAV:same-organizer-in-all-components`, RFC 6638 section 3.2.1.
    SameOrganizerInAllComponents,
    /// `CS:getctag`, the vendor property a poll for "did anything change" reads.
    Getctag,
}

/// Every row of the table, in declaration order.
///
/// A const array rather than a derived iterator, because [`ElementName::resolve`] scans it and
/// a test asserts that no two rows share a namespace and a local name — which is the property
/// that makes the scan's first hit the only hit.
const ALL: [ElementName; 77] = [
    ElementName::Multistatus,
    ElementName::Response,
    ElementName::Propstat,
    ElementName::Prop,
    ElementName::Href,
    ElementName::Status,
    ElementName::Error,
    ElementName::ResponseDescription,
    ElementName::Location,
    ElementName::Propfind,
    ElementName::Allprop,
    ElementName::Propname,
    ElementName::Include,
    ElementName::Getetag,
    ElementName::Getcontenttype,
    ElementName::Getcontentlength,
    ElementName::Getlastmodified,
    ElementName::Displayname,
    ElementName::Resourcetype,
    ElementName::Collection,
    ElementName::Principal,
    ElementName::Owner,
    ElementName::CurrentUserPrincipal,
    ElementName::PrincipalUrl,
    ElementName::SupportedReportSet,
    ElementName::SupportedReport,
    ElementName::Report,
    ElementName::SyncCollection,
    ElementName::SyncToken,
    ElementName::SyncLevel,
    ElementName::Limit,
    ElementName::Nresults,
    ElementName::ValidSyncToken,
    ElementName::NumberOfMatchesWithinLimits,
    ElementName::CalendarQuery,
    ElementName::CalendarMultiget,
    ElementName::FreeBusyQuery,
    ElementName::CalendarData,
    ElementName::Filter,
    ElementName::CompFilter,
    ElementName::PropFilter,
    ElementName::ParamFilter,
    ElementName::IsNotDefined,
    ElementName::TextMatch,
    ElementName::TimeRange,
    ElementName::CalendarDataComp,
    ElementName::CalendarDataAllcomp,
    ElementName::CalendarDataAllprop,
    ElementName::CalendarDataProp,
    ElementName::Expand,
    ElementName::LimitRecurrenceSet,
    ElementName::LimitFreebusySet,
    ElementName::Calendar,
    ElementName::CalendarHomeSet,
    ElementName::CalendarDescription,
    ElementName::CalendarTimezone,
    ElementName::SupportedCalendarComponentSet,
    ElementName::SupportedCalendarData,
    ElementName::MaxResourceSize,
    ElementName::MinDateTime,
    ElementName::MaxDateTime,
    ElementName::MaxInstances,
    ElementName::MaxAttendeesPerInstance,
    ElementName::SupportedFilter,
    ElementName::ValidCalendarData,
    ElementName::ValidCalendarObjectResource,
    ElementName::NoUidConflict,
    ElementName::CalendarUserAddressSet,
    ElementName::ScheduleInboxUrl,
    ElementName::ScheduleOutboxUrl,
    ElementName::ScheduleTag,
    ElementName::ScheduleCalendarTransp,
    ElementName::AllowedOrganizerSchedulingObjectChange,
    ElementName::AllowedAttendeeSchedulingObjectChange,
    ElementName::UniqueSchedulingObjectResource,
    ElementName::SameOrganizerInAllComponents,
    ElementName::Getctag,
];

impl ElementName {
    /// The row of the table a resolved name lands on, if any.
    ///
    /// `None` is a foreign element, which RFC 4918 section 17 requires a reader to tolerate
    /// and which [`crate::UnknownPolicy`] decides the fate of. It is never an error here,
    /// because whether a server's extension is skipped or refused is the caller's call and
    /// not this table's.
    #[must_use]
    pub fn resolve(namespace: Namespace<'_>, local_name: &[u8]) -> Option<Self> {
        ALL.into_iter().find(|candidate| {
            candidate.namespace().is(namespace) && candidate.local_name().as_bytes() == local_name
        })
    }

    /// The namespace and local name this row records.
    #[must_use]
    pub const fn spec(self) -> ElementSpec {
        ElementSpec {
            namespace: self.namespace(),
            local_name: self.local_name(),
        }
    }

    /// Whether this element's character data carries line endings that are its content.
    ///
    /// True for `CALDAV:calendar-data` and nothing else. An iCalendar object's `CRLF`
    /// terminators are RFC 5545 section 3.1 syntax rather than the file's layout, and XML 1.0
    /// section 2.11 would fold them away before any of this crate saw them. What this
    /// predicate scopes — a deliberate, stated, one-element departure from that rule — is
    /// [`crate::TextMode`]'s subject, and the reasoning is in `docs/adr/0004`.
    #[must_use]
    pub const fn preserves_line_endings(self) -> bool {
        matches!(self, Self::CalendarData)
    }

    /// Whether this build can honor the element.
    ///
    /// The table is unconditional and this is not: an RFC 6578 request in a build without
    /// `sync-collection` is refused as [`crate::DavError::Unsupported`], which is a different
    /// answer from skipping it as foreign. A server that skipped a `sync-collection` REPORT
    /// would answer it with a full enumeration and let the client believe it had synchronized.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        if cfg!(feature = "sync-collection") {
            true
        } else {
            !matches!(
                self,
                Self::SyncCollection | Self::SyncLevel | Self::Nresults
            )
        }
    }

    /// The namespace this element belongs to.
    #[must_use]
    pub const fn namespace(self) -> Namespace<'static> {
        match self {
            Self::Multistatus
            | Self::Response
            | Self::Propstat
            | Self::Prop
            | Self::Href
            | Self::Status
            | Self::Error
            | Self::ResponseDescription
            | Self::Location
            | Self::Propfind
            | Self::Allprop
            | Self::Propname
            | Self::Include
            | Self::Getetag
            | Self::Getcontenttype
            | Self::Getcontentlength
            | Self::Getlastmodified
            | Self::Displayname
            | Self::Resourcetype
            | Self::Collection
            | Self::Principal
            | Self::Owner
            | Self::CurrentUserPrincipal
            | Self::PrincipalUrl
            | Self::SupportedReportSet
            | Self::SupportedReport
            | Self::Report
            | Self::SyncCollection
            | Self::SyncToken
            | Self::SyncLevel
            | Self::Limit
            | Self::Nresults
            | Self::ValidSyncToken
            | Self::NumberOfMatchesWithinLimits => Namespace::Dav,
            Self::Getctag => Namespace::CalendarServer,
            Self::CalendarQuery
            | Self::CalendarMultiget
            | Self::FreeBusyQuery
            | Self::CalendarData
            | Self::Filter
            | Self::CompFilter
            | Self::PropFilter
            | Self::ParamFilter
            | Self::IsNotDefined
            | Self::TextMatch
            | Self::TimeRange
            | Self::CalendarDataComp
            | Self::CalendarDataAllcomp
            | Self::CalendarDataAllprop
            | Self::CalendarDataProp
            | Self::Expand
            | Self::LimitRecurrenceSet
            | Self::LimitFreebusySet
            | Self::Calendar
            | Self::CalendarHomeSet
            | Self::CalendarDescription
            | Self::CalendarTimezone
            | Self::SupportedCalendarComponentSet
            | Self::SupportedCalendarData
            | Self::MaxResourceSize
            | Self::MinDateTime
            | Self::MaxDateTime
            | Self::MaxInstances
            | Self::MaxAttendeesPerInstance
            | Self::SupportedFilter
            | Self::ValidCalendarData
            | Self::ValidCalendarObjectResource
            | Self::NoUidConflict
            | Self::CalendarUserAddressSet
            | Self::ScheduleInboxUrl
            | Self::ScheduleOutboxUrl
            | Self::ScheduleTag
            | Self::ScheduleCalendarTransp
            | Self::AllowedOrganizerSchedulingObjectChange
            | Self::AllowedAttendeeSchedulingObjectChange
            | Self::UniqueSchedulingObjectResource
            | Self::SameOrganizerInAllComponents => Namespace::CalDav,
        }
    }

    /// The element's local name, exactly as its specification spells it.
    ///
    /// Case included: RFC 3744 writes `principal-URL` and RFC 6638 writes `schedule-inbox-URL`
    /// with those capitals, and an XML local name is case-sensitive, so lowercasing them here
    /// would name elements no server sends.
    #[must_use]
    pub const fn local_name(self) -> &'static str {
        match self {
            Self::Multistatus => "multistatus",
            Self::Response => "response",
            Self::Propstat => "propstat",
            Self::Prop | Self::CalendarDataProp => "prop",
            Self::Href => "href",
            Self::Status => "status",
            Self::Error => "error",
            Self::ResponseDescription => "responsedescription",
            Self::Location => "location",
            Self::Propfind => "propfind",
            Self::Allprop | Self::CalendarDataAllprop => "allprop",
            Self::Propname => "propname",
            Self::Include => "include",
            Self::Getetag => "getetag",
            Self::Getcontenttype => "getcontenttype",
            Self::Getcontentlength => "getcontentlength",
            Self::Getlastmodified => "getlastmodified",
            Self::Displayname => "displayname",
            Self::Resourcetype => "resourcetype",
            Self::Collection => "collection",
            Self::Principal => "principal",
            Self::Owner => "owner",
            Self::CurrentUserPrincipal => "current-user-principal",
            Self::PrincipalUrl => "principal-URL",
            Self::SupportedReportSet => "supported-report-set",
            Self::SupportedReport => "supported-report",
            Self::Report => "report",
            Self::SyncCollection => "sync-collection",
            Self::SyncToken => "sync-token",
            Self::SyncLevel => "sync-level",
            Self::Limit => "limit",
            Self::Nresults => "nresults",
            Self::ValidSyncToken => "valid-sync-token",
            Self::NumberOfMatchesWithinLimits => "number-of-matches-within-limits",
            Self::CalendarQuery => "calendar-query",
            Self::CalendarMultiget => "calendar-multiget",
            Self::FreeBusyQuery => "free-busy-query",
            Self::CalendarData => "calendar-data",
            Self::Filter => "filter",
            Self::CompFilter => "comp-filter",
            Self::PropFilter => "prop-filter",
            Self::ParamFilter => "param-filter",
            Self::IsNotDefined => "is-not-defined",
            Self::TextMatch => "text-match",
            Self::TimeRange => "time-range",
            Self::CalendarDataComp => "comp",
            Self::CalendarDataAllcomp => "allcomp",
            Self::Expand => "expand",
            Self::LimitRecurrenceSet => "limit-recurrence-set",
            Self::LimitFreebusySet => "limit-freebusy-set",
            Self::Calendar => "calendar",
            Self::CalendarHomeSet => "calendar-home-set",
            Self::CalendarDescription => "calendar-description",
            Self::CalendarTimezone => "calendar-timezone",
            Self::SupportedCalendarComponentSet => "supported-calendar-component-set",
            Self::SupportedCalendarData => "supported-calendar-data",
            Self::MaxResourceSize => "max-resource-size",
            Self::MinDateTime => "min-date-time",
            Self::MaxDateTime => "max-date-time",
            Self::MaxInstances => "max-instances",
            Self::MaxAttendeesPerInstance => "max-attendees-per-instance",
            Self::SupportedFilter => "supported-filter",
            Self::ValidCalendarData => "valid-calendar-data",
            Self::ValidCalendarObjectResource => "valid-calendar-object-resource",
            Self::NoUidConflict => "no-uid-conflict",
            Self::CalendarUserAddressSet => "calendar-user-address-set",
            Self::ScheduleInboxUrl => "schedule-inbox-URL",
            Self::ScheduleOutboxUrl => "schedule-outbox-URL",
            Self::ScheduleTag => "schedule-tag",
            Self::ScheduleCalendarTransp => "schedule-calendar-transp",
            Self::AllowedOrganizerSchedulingObjectChange => {
                "allowed-organizer-scheduling-object-change"
            },
            Self::AllowedAttendeeSchedulingObjectChange => {
                "allowed-attendee-scheduling-object-change"
            },
            Self::UniqueSchedulingObjectResource => "unique-scheduling-object-resource",
            Self::SameOrganizerInAllComponents => "same-organizer-in-all-components",
            Self::Getctag => "getctag",
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{ALL, ElementName, Namespace, QName};

    #[test]
    fn a_prefix_is_never_what_a_name_resolves_on() {
        // The three spellings the deployed world actually sends. All of them are one element,
        // and the fourth is a different one however familiar its prefix looks.
        let lower = QName::new(Namespace::from_uri(b"DAV:"), b"multistatus");
        let upper = QName::new(Namespace::from_uri(b"DAV:"), b"multistatus");
        let hostile = QName::new(
            Namespace::from_uri(b"http://evil.example/not-dav"),
            b"multistatus",
        );
        assert_eq!(lower.known(), Some(ElementName::Multistatus));
        assert_eq!(upper.known(), Some(ElementName::Multistatus));
        assert_eq!(hostile.known(), None);
    }

    #[test]
    fn one_local_name_in_two_namespaces_is_two_elements() {
        assert_eq!(
            ElementName::resolve(Namespace::Dav, b"prop"),
            Some(ElementName::Prop)
        );
        assert_eq!(
            ElementName::resolve(Namespace::CalDav, b"prop"),
            Some(ElementName::CalendarDataProp)
        );
    }

    #[test]
    fn no_two_rows_share_a_namespace_and_a_local_name() {
        for (index, row) in ALL.into_iter().enumerate() {
            for other in ALL.into_iter().skip(index.saturating_add(1)) {
                assert!(
                    !(row.namespace().is(other.namespace())
                        && row.local_name() == other.local_name()),
                    "{row:?} and {other:?} are the same name"
                );
            }
        }
    }

    #[test]
    fn every_row_resolves_back_to_itself() {
        for row in ALL {
            let found = ElementName::resolve(row.namespace(), row.local_name().as_bytes());
            assert_eq!(found, Some(row), "{row:?}");
        }
    }

    #[test]
    fn one_element_and_only_one_keeps_its_line_endings() {
        let preserving: Vec<ElementName> = ALL
            .into_iter()
            .filter(|row| row.preserves_line_endings())
            .collect();
        assert_eq!(preserving, [ElementName::CalendarData]);
    }

    #[test]
    fn a_namespace_uri_is_compared_octet_for_octet() {
        assert_eq!(Namespace::from_uri(b"DAV:"), Namespace::Dav);
        assert_eq!(Namespace::from_uri(b"dav:"), Namespace::Other(b"dav:"));
    }
}
