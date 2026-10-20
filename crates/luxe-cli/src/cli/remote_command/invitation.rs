//! The invitation a phone scans: a QR code in the terminal, and the same link in words.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use luxe_remote::{DeviceBook, Tailnet};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;

use crate::output::outln;

/// `luxe remote pair`: one invitation, shown.
pub(super) async fn pair(port: u16, devices: &DeviceBook) -> anyhow::Result<()> {
    match luxe_remote::this_machine().await {
        Ok(tailnet) => show(port, devices, &tailnet),
        Err(error) => {
            let token = devices.invite()?;
            outln!("Tailscale couldn't name this machine ({error}).");
            outln!("Open /pair on the bridge's tailnet address and add #{token} to it.");
            Ok(())
        }
    }
}

/// Issues an invitation and shows it as a QR code and a link to `https://<this machine>/pair`.
///
/// The invitation rides in the link's fragment, which a browser never sends to a server, so it
/// stays out of Serve's and the bridge's logs.
pub(super) fn show(port: u16, devices: &DeviceBook, tailnet: &Tailnet) -> anyhow::Result<()> {
    let token = devices.invite()?;
    let link = format!("https://{}/pair#{token}", tailnet.dns_name);
    if let Some(code) = qr(&link) {
        outln!("{code}");
    }
    outln!("On the phone or tablet: install Tailscale, sign in as {}, then", tailnet.login);
    outln!("scan this or open the link. It pairs one device, within ten minutes.\n");
    outln!("  {link}\n");
    if !bridge_listening(port) {
        outln!("The bridge isn't running yet: start it with `luxe remote` before scanning.");
    }
    let _ = std::io::stdout().flush();
    Ok(())
}

/// `text` as a QR code drawn in half-height blocks, light modules as blocks so it reads on the
/// dark background most terminals have.
fn qr(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let drawn = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Some(drawn)
}

/// Whether something answers on the bridge's port.
pub(super) fn bridge_listening(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code is drawn with modules as blocks and a quiet zone a camera can find.
    ///
    /// The break this catches: losing the quiet zone or the inversion, either of which leaves a
    /// code most phone cameras won't pick out of a dark terminal.
    #[test]
    fn the_code_has_a_quiet_zone_of_blocks() {
        let code = qr("https://desk.tail1234.ts.net/pair#abc").unwrap();
        let first = code.lines().next().unwrap();
        assert!(first.chars().all(|c| c == '\u{2588}'), "the border is solid: {first:?}");
        assert!(code.lines().count() > 10);
    }
}
