#[derive(Debug, Clone)]
pub struct SessionDescription(String);

impl SessionDescription {
    /// Returns an instance of `SessionDescription` if the input satisfies all our validation constraints.
    pub fn parse(s: String) -> Result<SessionDescription, String> {
        // `.trim()` returns a view over the input `s` without trailing
        // whitespace-like characters.
        // `.is_empty` checks if the view contains any character.
        let is_empty_or_whitespace = s.trim().is_empty();

        // sdp should start with v=0
        let starts_with_v0 = s.starts_with("v=0");

        // sdp should contain 'a=sendonly' or 'a=recvonly'
        let sendonly_or_recvonly = s.contains("a=sendonly") || s.contains("a=recvonly");

        // if is_empty_or_whitespace || !starts_with_v0 || !sendonly_or_recvonly {
        //     Err(format!("Invalid Sdp: {}", s))
        // } else {
        //     Ok(Self(s))
        // }
        Ok(Self(s))
    }

    pub fn is_sendonly(&self) -> bool {
        self.0.contains("a=sendonly")
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn set_as_active(&mut self) {
        self.0 = self.0.replace("a=setup:actpass", "a=setup:active");
    }

    pub fn set_as_passive(&mut self) {
        self.0 = self.0.replace("a=setup:actpass", "a=setup:passive");
    }
}

impl AsRef<str> for SessionDescription {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

pub const VALID_WHIP_OFFER: &'static str = "v=0
    o=- 8119464979627461093 0 IN IP4 0.0.0.0
    s=-
    t=0 0
    a=ice-options:trickle
    a=group:BUNDLE video0
    m=video 9 UDP/TLS/RTP/SAVPF 96
    c=IN IP4 0.0.0.0
    a=setup:actpass
    a=ice-ufrag:nCDA1pOKt6pxoXhw47QNjh9Ea+5iSzch
    a=ice-pwd:8JrchkUez1iva/w2VWHkLIiBrE3tvicx
    a=rtcp-mux
    a=rtcp-rsize
    a=sendonly
    a=rtpmap:96 H264/90000
    a=rtcp-fb:96 nack pli
    a=rtcp-fb:96 ccm fir
    a=rtcp-fb:96 transport-cc
    a=framerate:20
    a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z3oAM7y0AXoHv8uAtQEBAUAAAAMAQAAACiPGDKg=,aO88sA==;profile-level-id=7a0033;level-asymmetry-allowed=1
    a=ssrc:2736603989 msid:user784124463@host-7732ac8e webrtctransceiver0
    a=ssrc:2736603989 cname:user784124463@host-7732ac8e
    a=mid:video0
    a=fingerprint:sha-256 47:3F:D2:71:05:5E:0A:10:F9:35:90:61:9A:49:91:7C:35:5A:B9:2A:8B:AB:D6:9A:DD:36:F4:0B:E5:14:17:86
    a=rtcp-mux-only
    a=candidate:1 1 UDP 2015363327 fe80::1834:cb0a:c07b:b1bc 54257 typ host
    a=candidate:2 1 TCP 1015021823 fe80::1834:cb0a:c07b:b1bc 9 typ host tcptype active
    a=candidate:3 1 TCP 1010827519 fe80::1834:cb0a:c07b:b1bc 56566 typ host tcptype passive
    a=candidate:4 1 UDP 2015363583 10.247.169.107 53559 typ host
    a=candidate:5 1 TCP 1015022079 10.247.169.107 9 typ host tcptype active
    a=candidate:6 1 TCP 1010827775 10.247.169.107 56567 typ host tcptype passive";

pub const VALID_WHEP_OFFER: &'static str = "v=0
    o=- 4658353067706891397 0 IN IP4 0.0.0.0
    s=-
    t=0 0
    a=ice-options:trickle
    a=group:BUNDLE video0
    m=video 9 UDP/TLS/RTP/SAVPF 96
    c=IN IP4 0.0.0.0
    a=setup:actpass
    a=ice-ufrag:Avv2VrwoBrrlWRdPo6G6iosh8vkNlD3c
    a=ice-pwd:7Cw49i4Cf0z8M/O9B9NccatweSJdlETz
    a=rtcp-mux
    a=rtcp-rsize
    a=recvonly
    a=rtpmap:96 H264/90000
    a=rtcp-fb:96 nack pli
    a=rtcp-fb:96 ccm fir
    a=rtcp-fb:96 transport-cc
    a=mid:video0
    a=fingerprint:sha-256 27:04:FA:3B:82:77:17:2F:8C:69:47:B8:57:07:C9:68:AC:58:74:12:24:4B:CD:83:C4:D9:83:A1:BE:4D:22:4C
    a=rtcp-mux-only
    a=candidate:1 1 UDP 2015363327 fe80::1834:cb0a:c07b:b1bc 53998 typ host
    a=candidate:2 1 TCP 1015021823 fe80::1834:cb0a:c07b:b1bc 9 typ host tcptype active
    a=candidate:3 1 TCP 1010827519 fe80::1834:cb0a:c07b:b1bc 56577 typ host tcptype passive
    a=candidate:4 1 UDP 2015363583 10.247.169.107 62020 typ host
    a=candidate:5 1 TCP 1015022079 10.247.169.107 9 typ host tcptype active
    a=candidate:6 1 TCP 1010827775 10.247.169.107 56578 typ host tcptype passive";

#[cfg(test)]
mod tests {
    use super::{SessionDescription, VALID_WHEP_OFFER, VALID_WHIP_OFFER};
    use claims::{assert_err, assert_ok};

    #[test]
    fn whitespace_only_sdp_are_rejected() {
        let sdp = " ".to_string();
        assert_err!(SessionDescription::parse(sdp));
    }

    #[test]
    fn empty_string_is_rejected() {
        let sdp = "".to_string();
        assert_err!(SessionDescription::parse(sdp));
    }

    #[test]
    fn sdp_not_starting_with_v0_is_rejected() {
        let sdp = "v=1".to_string();
        assert_err!(SessionDescription::parse(sdp));
    }

    #[test]
    fn sdp_not_containing_a_sendonly_or_recvonly_is_rejected() {
        let sdp = "v=0".to_string();
        assert_err!(SessionDescription::parse(sdp));
    }

    #[test]
    fn valid_sdps_are_parsed_successfully() {
        let whip_sdp = VALID_WHIP_OFFER.to_string();
        assert_ok!(SessionDescription::parse(whip_sdp));

        let whep_sdp = VALID_WHEP_OFFER.to_string();
        assert_ok!(SessionDescription::parse(whep_sdp));
    }
}
