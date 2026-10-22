//! Embedded posts and widgets from social networks. They track every page they appear on,
//! whether or not you have an account, so they are blocked as third parties, as Brave does.
//! Sign-in buttons are left alone: they only act when clicked.

/// Which networks' embeds are blocked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Social {
    pub facebook: bool,
    pub x: bool,
    pub linkedin: bool,
}

const FACEBOOK: &str = "\
||facebook.com/plugins/$third-party,domain=~facebook.com|~messenger.com|~instagram.com
||facebook.com/v*/plugins/$third-party,domain=~facebook.com|~messenger.com|~instagram.com
||facebook.net^*/xfbml.$third-party,domain=~facebook.com|~messenger.com|~instagram.com
||facebook.com/tr^$third-party,domain=~facebook.com|~messenger.com|~instagram.com
@@||facebook.com/plugins/login_button.php$third-party,domain=~facebook.com|~messenger.com|~instagram.com
@@||facebook.com/v*/plugins/login_button.php$third-party,domain=~facebook.com|~messenger.com|~instagram.com
~facebook.com##iframe[src*=\"facebook.com/plugins/\"]:not([src*=\"login_button\"])
~facebook.com##iframe[src*=\"facebook.com/v\"][src*=\"/plugins/\"]:not([src*=\"login_button\"])
";

const X: &str = "\
||platform.twitter.com/widgets.js$third-party,domain=~x.com|~twitter.com
||platform.twitter.com/widgets/$third-party,domain=~x.com|~twitter.com
||platform.twitter.com/embed/$third-party,domain=~x.com|~twitter.com
||platform.x.com^$third-party,domain=~x.com|~twitter.com
||syndication.twitter.com^$third-party,domain=~x.com|~twitter.com
||syndication.x.com^$third-party,domain=~x.com|~twitter.com
||cdn.syndication.twimg.com^$third-party,domain=~x.com|~twitter.com
~twitter.com,~x.com##iframe[src*=\"platform.twitter.com/embed\"]
";

const LINKEDIN: &str = "\
||platform.linkedin.com^$third-party,domain=~linkedin.com
||linkedin.com/embed/$third-party,domain=~linkedin.com
||linkedin.com/pages-extensions/$third-party,domain=~linkedin.com
||badges.linkedin.com^$third-party,domain=~linkedin.com
~linkedin.com##iframe[src*=\"linkedin.com/embed/\"]
";

/// Filter rules for the networks that are switched on.
pub fn rules(s: Social) -> String {
    [(s.facebook, FACEBOOK), (s.x, X), (s.linkedin, LINKEDIN)]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, r)| *r)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decision, Mode, RequestKind, Shield};

    fn shield(rules: &str) -> Shield {
        let s = Shield::new();
        assert!(s.load(vec![(rules.to_string(), true)], &[]));
        s
    }

    #[test]
    fn embeds_are_blocked_and_sign_in_is_not() {
        let all = Social {
            facebook: true,
            x: true,
            linkedin: true,
        };
        let s = shield(&rules(all));
        let page = "https://news.test/story";
        let check = |url: &str, kind| s.check(url, page, kind, Mode::Standard);
        let frame = RequestKind::Subdocument;
        for blocked in [
            "https://www.facebook.com/plugins/post.php?href=x",
            "https://www.facebook.com/v18.0/plugins/like.php?href=x",
            "https://platform.twitter.com/embed/Tweet.html?id=1",
            "https://www.linkedin.com/embed/feed/update/urn:li:share:1",
        ] {
            assert_eq!(check(blocked, frame), Decision::Block, "{blocked}");
        }
        for blocked in [
            "https://platform.twitter.com/widgets.js",
            "https://platform.linkedin.com/badges/js/profile.js",
            "https://connect.facebook.net/en_US/sdk/xfbml.customerchat.js",
        ] {
            assert_eq!(
                check(blocked, RequestKind::Script),
                Decision::Block,
                "{blocked}"
            );
        }
        // Signing in with Facebook keeps working: its SDK and login button load.
        assert_eq!(
            check(
                "https://www.facebook.com/plugins/login_button.php?x=1",
                frame
            ),
            Decision::Allow
        );
        assert_eq!(
            check(
                "https://connect.facebook.net/en_US/sdk.js",
                RequestKind::Script
            ),
            Decision::Allow
        );
        // The networks' own sites are untouched.
        assert_eq!(
            s.check(
                "https://platform.twitter.com/embed/Tweet.html?id=1",
                "https://x.com/home",
                frame,
                Mode::Standard
            ),
            Decision::Allow
        );
    }

    #[test]
    fn only_switched_on_networks_get_rules() {
        assert!(rules(Social::default()).is_empty());
        let x = rules(Social {
            x: true,
            ..Social::default()
        });
        assert!(x.contains("twitter") && !x.contains("facebook") && !x.contains("linkedin"));
    }
}
