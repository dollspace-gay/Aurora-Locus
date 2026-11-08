//! The `tools.kryphocron.*` NSIDs and how locus routes them.

use kryphocron::Tier;

pub const PREFIX: &str = "tools.kryphocron.";

pub const POST_PUBLIC: &str = "tools.kryphocron.feed.postPublic";
pub const POST_PRIVATE: &str = "tools.kryphocron.feed.postPrivate";
pub const LIKE: &str = "tools.kryphocron.feed.like";
pub const REPOST: &str = "tools.kryphocron.feed.repost";
pub const THREADGATE: &str = "tools.kryphocron.feed.threadgate";
pub const BLOCK: &str = "tools.kryphocron.graph.block";
pub const MUTE: &str = "tools.kryphocron.graph.mute";
pub const AUDIENCE: &str = "tools.kryphocron.policy.audience";

pub const CREATE_POST_PRIVATE: &str = "tools.kryphocron.feed.createPostPrivate";
pub const UPDATE_POST_PRIVATE: &str = "tools.kryphocron.feed.updatePostPrivate";
pub const DELETE_POST_PRIVATE: &str = "tools.kryphocron.feed.deletePostPrivate";
pub const PARTICIPATE_PRIVATE: &str = "tools.kryphocron.actor.participatePrivate";
pub const MANAGE_AUDIENCE: &str = "tools.kryphocron.policy.manageAudience";
pub const DELETE_AUDIENCE: &str = "tools.kryphocron.policy.deleteAudience";

/// The codec locus encodes with and accepts.
pub const CODEC: &str = "laquna/0.2";

/// What a collection is to locus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Not a kryphocron collection: a normal record.
    Other,
    /// A registered kryphocron record collection.
    Record { private: bool },
    /// A `tools.kryphocron.*` collection the registry doesn't know: refused
    /// on every write path.
    Unregistered,
}

/// Classifies a collection through the registry (`Tier::from_nsid`).
#[must_use]
pub fn classify(collection: &str) -> Kind {
    if !collection.starts_with(PREFIX) {
        return Kind::Other;
    }
    let Ok(nsid) = kryphocron::Nsid::new(collection) else {
        return Kind::Unregistered;
    };
    match Tier::from_nsid(&nsid) {
        Ok(Tier::Public) => Kind::Record { private: false },
        // Any tier other than public (only private today) is treated as
        // private: the stricter rules.
        Ok(_) => Kind::Record { private: true },
        Err(_) => Kind::Unregistered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_decides() {
        assert_eq!(classify("app.bsky.feed.post"), Kind::Other);
        assert_eq!(classify(POST_PUBLIC), Kind::Record { private: false });
        assert_eq!(classify(THREADGATE), Kind::Record { private: false });
        for c in [POST_PRIVATE, BLOCK, MUTE, AUDIENCE] {
            assert_eq!(classify(c), Kind::Record { private: true }, "{c}");
        }
        assert_eq!(
            classify("tools.kryphocron.feed.nothing"),
            Kind::Unregistered
        );
        assert_eq!(
            classify(CREATE_POST_PRIVATE),
            Kind::Unregistered,
            "procedures carry no tier"
        );
    }
}
