use crate::transport::slurp_url;
use common::log;
use derive_more::Display;
use gstuff::try_s;
use gstuff::{ERR, ERRL};
use mm2_core::mm_ctx::MmArc;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::fs;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use mm2_err_handle::prelude::{MapToMmResult, MmError};
use std::net::ToSocketAddrs;

const IP_PROVIDERS: [&str; 2] = ["http://checkip.amazonaws.com/", "http://api.ipify.org"];

/// Tries to serve on the given IP to check if it's available.  
/// We need this check because our external IP, particularly under NAT,
/// might be outside of the set of IPs we can open and run a server on.
///
/// Returns an error if the address did not work
/// (like when the `ip` does not belong to a connected interface).
///
/// The primary concern of this function is to test the IP,
/// but this opportunity is also used to start the HTTP fallback server,
/// in order to improve the reliability of the said server (in the Lean "stop the line" manner).
///
/// If the IP has passed the communication check then a shutdown Sender is returned.
/// Dropping or using that Sender will stop the HTTP fallback server.
///
/// Also the port of the HTTP fallback server is returned.
fn test_ip(ctx: &MmArc, ip: IpAddr) -> Result<(), String> {
    let netid = ctx.netid();

    // Try a few pseudo-random ports.
    // `netid` is used as the seed in order for the port selection to be determenistic,
    // similar to how the port selection and probing worked before (since MM1)
    // and in order to reduce the likehood of *unexpected* port conflicts.
    let mut attempts_left = 9;
    let mut rng = SmallRng::seed_from_u64(netid as u64);
    loop {
        if attempts_left < 1 {
            break ERR!("Out of attempts");
        }
        attempts_left -= 1;
        // TODO: Avoid `mypubport`.
        let port = rng.gen_range(1111, 65535);
        log::info!("Trying to bind on {}:{}", ip, port);
        match std::net::TcpListener::bind((ip, port)) {
            Ok(_) => break Ok(()),
            Err(err) => {
                if attempts_left == 0 {
                    break ERR!("{}", err);
                }
                continue;
            },
        }
    }
}

fn simple_ip_extractor(ip: &str) -> Result<IpAddr, String> {
    let ip = ip.trim();
    Ok(match ip.parse() {
        Ok(ip) => ip,
        Err(err) => return ERR!("Error parsing IP address '{}': {}", ip, err),
    })
}

/// Detect the outer IP address, visible to the internet.
pub async fn fetch_external_ip() -> Result<IpAddr, String> {
    for url in IP_PROVIDERS.iter() {
        log::info!("Trying to fetch the real IP from '{}' ...", url);
        let (status, _headers, ip) = match slurp_url(url).await {
            Ok(t) => t,
            Err(err) => {
                log::error!("Failed to fetch IP from '{}': {}", url, err);
                continue;
            },
        };
        if !status.is_success() {
            log::error!("Failed to fetch IP from '{}': status {:?}", url, status);
            continue;
        }
        let ip = match std::str::from_utf8(&ip) {
            Ok(ip) => ip,
            Err(err) => {
                log::error!("Failed to fetch IP from '{}', not UTF-8: {}", url, err);
                continue;
            },
        };
        match simple_ip_extractor(ip) {
            Ok(ip) => return Ok(ip),
            Err(err) => {
                log::error!("Failed to parse IP '{}' fetched from '{}': {}", ip, url, err);
                continue;
            },
        };
    }
    ERR!("Couldn't fetch the real IP")
}

/// Detect the real IP address.
///
/// We're detecting the outer IP address, visible to the internet.
/// Later we'll try to *bind* on this IP address,
/// and this will break under NAT or forwarding because the internal IP address will be different.
/// Which might be a good thing, allowing us to detect the likehoodness of NAT early.
async fn detect_myipaddr(ctx: MmArc) -> Result<IpAddr, String> {
    let ip = try_s!(fetch_external_ip().await);

    // Try to bind on this IP.
    // If we're not behind a NAT then the bind will likely succeed.
    // If the bind fails then emit a user-visible warning and fall back to 0.0.0.0.
    match test_ip(&ctx, ip) {
        Ok(_) => {
            let msg = format!("We've detected an external IP {ip} and we can bind on it, so probably a dedicated IP.");
            ctx.log.log("🙂", &[&"myipaddr"], &msg);
            return Ok(ip);
        },
        Err(err) => log::error!("IP {} not available: {}", ip, err),
    }
    let all_interfaces = Ipv4Addr::new(0, 0, 0, 0).into();
    if test_ip(&ctx, all_interfaces).is_ok() {
        let error =
            format!("We couldn't bind on the external IP {ip}, so NAT is likely to be present. We'll be okay though.");
        ctx.log.log("😅", &[&"myipaddr"], &error);
        return Ok(all_interfaces);
    }
    let localhost = Ipv4Addr::new(127, 0, 0, 1).into();
    if test_ip(&ctx, localhost).is_ok() {
        let error = format!(
            "We couldn't bind on {ip} or 0.0.0.0! Looks like we can bind on 127.0.0.1 as a workaround, but that's not how we're supposed to work."
        );
        ctx.log.log("🤫", &[&"myipaddr"], &error);
        return Ok(localhost);
    }

    let error = format!("Couldn't bind on {ip}, 0.0.0.0 or 127.0.0.1.");
    ctx.log.log("🤒", &[&"myipaddr"], &error);
    // Seems like a better default than 127.0.0.1, might still work for other ports.
    Ok(all_interfaces)
}

pub async fn myipaddr(ctx: MmArc) -> Result<IpAddr, String> {
    let myipaddr: IpAddr = if Path::new("myipaddr").exists() {
        match fs::File::open("myipaddr") {
            Ok(mut f) => {
                let mut buf = String::new();
                if let Err(err) = f.read_to_string(&mut buf) {
                    return ERR!("Can't read from 'myipaddr': {}", err);
                }
                try_s!(simple_ip_extractor(&buf))
            },
            Err(err) => return ERR!("Can't read from 'myipaddr': {}", err),
        }
    } else if !ctx.conf["myipaddr"].is_null() {
        let s = try_s!(ctx.conf["myipaddr"].as_str().ok_or("'myipaddr' is not a string"));
        try_s!(simple_ip_extractor(s))
    } else {
        try_s!(detect_myipaddr(ctx).await)
    };
    Ok(myipaddr)
}

#[derive(Debug, Display)]
pub enum ParseAddressError {
    #[display(fmt = "Address '{address}' cannot be resolved to IPv4.")]
    CannotResolveIPv4 { address: String },
    #[display(fmt = "Couldn't resolve any IP on '{address}' address. {reason}")]
    UnresolvedAddress { address: String, reason: String },
}

pub fn addr_to_ipv4_string(address: &str) -> Result<String, MmError<ParseAddressError>> {
    // Remove "https:// or http://" etc.. from address str
    let formated_address = address.split("://").last().unwrap_or(address);

    let address_with_port = format!(
        "{formated_address}{}",
        if formated_address.contains(':') { "" } else { ":0" }
    );

    let iter = address_with_port.as_str().to_socket_addrs().map_to_mm(|e| {
        log::error!("Couldn't resolve '{}' seed: {}", address, e);
        ParseAddressError::UnresolvedAddress {
            address: address.to_owned(),
            reason: e.to_string(),
        }
    })?;

    if iter.len() == 0 {
        return MmError::err(ParseAddressError::UnresolvedAddress {
            address: address.to_owned(),
            reason: "Empty DNS result.".to_owned(),
        });
    }

    for resolved in iter {
        if resolved.is_ipv4() {
            return Ok(resolved.ip().to_string());
        } else {
            log::warn!(
                "Address/Seed {} resolved to IPv6 {} which is not supported",
                address,
                resolved
            );
        }
    }

    MmError::err(ParseAddressError::CannotResolveIPv4 {
        address: address.to_owned(),
    })
}

/// Stable port of `Ipv4Addr::is_global` which should be removed once
/// stabilized on std.
pub fn is_global_ipv4(ip: &Ipv4Addr) -> bool {
    !(ip.octets()[0] == 0 // "This network"
            || ip.is_private()
            || ip.octets()[0] == 100 && (ip.octets()[1] & 0b1100_0000 == 0b0100_0000)
            || ip.is_loopback()
            || ip.is_link_local()
            // addresses reserved for future protocols (`192.0.0.0/24`)
            // .9 and .10 are documented as globally reachable so they're excluded
            || (
                ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0
                && ip.octets()[3] != 9 && ip.octets()[3] != 10
            )
            || ip.is_documentation()
            || ip.octets()[0] == 198 && (ip.octets()[1] & 0xfe) == 18
            || ip.octets()[0] & 240 == 240 && !ip.is_broadcast()
            || ip.is_broadcast())
}
