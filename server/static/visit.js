// Counts this page view (see server/src/visits.rs): the server records the
// time, the site, the visitor's approximate country, and a daily visitor
// hash, never the address.
navigator.sendBeacon?.("/visit");
