//! Canonical names for Byōbu's CNAME uncloaking, from Windows' own resolver (and its cache,
//! which the page's own lookup of the same name then reuses).

use shoji_byobu::Resolution;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::DNS_ERROR_RCODE_NAME_ERROR;
use windows::Win32::NetworkManagement::Dns::{
    DnsFree, DnsFreeRecordList, DnsQueryEx, DNS_QUERY_NO_MULTICAST, DNS_QUERY_NO_NETBT,
    DNS_QUERY_REQUEST, DNS_QUERY_REQUEST_VERSION1, DNS_QUERY_RESULT, DNS_RECORDW, DNS_TYPE_A,
    DNS_TYPE_CNAME,
};

/// Look `host` up and follow its CNAME chain. Blocking: called on Byōbu's resolver threads.
pub fn resolve(host: &str) -> Option<Resolution> {
    let name: Vec<u16> = host.encode_utf16().chain(Some(0)).collect();
    let request = DNS_QUERY_REQUEST {
        Version: DNS_QUERY_REQUEST_VERSION1.0,
        QueryName: PCWSTR(name.as_ptr()),
        QueryType: DNS_TYPE_A.0,
        QueryOptions: (DNS_QUERY_NO_MULTICAST.0 | DNS_QUERY_NO_NETBT.0) as u64,
        ..Default::default()
    };
    let mut result = DNS_QUERY_RESULT {
        Version: DNS_QUERY_REQUEST_VERSION1.0,
        ..Default::default()
    };
    // With no completion callback the query completes before it returns.
    let status = unsafe { DnsQueryEx(&request, &mut result, None) };
    let records = result.pQueryRecords as *mut DNS_RECORDW;
    if status != 0 || result.QueryStatus != 0 {
        if !records.is_null() {
            unsafe { DnsFree(Some(records as *const _), DnsFreeRecordList) };
        }
        // A name that doesn't exist has nothing to hide; anything else is tried again later.
        return (result.QueryStatus == DNS_ERROR_RCODE_NAME_ERROR.0 as i32).then_some(Resolution {
            canonical: None,
            ttl: Duration::from_secs(600),
        });
    }
    let mut canonical = None;
    let mut ttl = u32::MAX;
    let mut r = records;
    while !r.is_null() {
        let rec = unsafe { &*r };
        ttl = ttl.min(rec.dwTtl);
        if rec.wType == DNS_TYPE_CNAME.0 {
            if let Ok(target) = unsafe { rec.Data.CNAME.pNameHost.to_string() } {
                canonical = Some(target);
            }
        }
        r = rec.pNext;
    }
    if !records.is_null() {
        unsafe { DnsFree(Some(records as *const _), DnsFreeRecordList) };
    }
    Some(Resolution {
        canonical,
        ttl: Duration::from_secs(if ttl == u32::MAX { 600 } else { ttl as u64 }),
    })
}
