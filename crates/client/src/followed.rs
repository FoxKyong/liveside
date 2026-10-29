//! The followed channels, as both frontends show them: a stream in a few
//! words, and the list kept up to date from what the engine hears every
//! minute.

use liveside_proto::model::{FollowedChannel, LiveStream};

use crate::lang::Strings;

/// What a channel streams, in one line: "Art · Cozy pixel art". A stream
/// without a title says so rather than showing nothing.
pub fn about(stream: &LiveStream, text: &Strings) -> String {
    let title = match stream.title.trim() {
        "" => text.followed_no_title,
        title => title,
    };
    match stream.category.trim() {
        "" => title.to_owned(),
        category => format!("{category} · {title}"),
    }
}

/// The list as it stands now: `live` is every followed channel streaming,
/// so the ones in `list` that are not in it have stopped. Live ones come
/// first in `live`'s order (the most watched first), then the rest by
/// login; a channel streaming that `list` did not have — followed since
/// — is added.
pub fn with_live(list: &[FollowedChannel], live: &[FollowedChannel]) -> Vec<FollowedChannel> {
    let mut now: Vec<FollowedChannel> = live.to_vec();
    let mut rest: Vec<FollowedChannel> = list
        .iter()
        .filter(|f| !live.iter().any(|l| l.channel.id == f.channel.id))
        .map(|f| FollowedChannel {
            channel: f.channel.clone(),
            live: None,
        })
        .collect();
    rest.sort_by(|a, b| a.channel.login.as_str().cmp(b.channel.login.as_str()));
    now.extend(rest);
    now
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::Timestamp;
    use liveside_proto::model::{Channel, StreamStats};

    fn followed(login: &str, viewers: Option<u32>) -> FollowedChannel {
        FollowedChannel {
            channel: Channel {
                id: format!("id-{login}").into(),
                login: login.into(),
                display_name: login.into(),
            },
            live: viewers.map(|viewers| LiveStream {
                title: "t".into(),
                category: String::new(),
                stats: StreamStats {
                    viewers,
                    started: Timestamp(0),
                },
            }),
        }
    }

    fn shown(list: &[FollowedChannel]) -> Vec<(&str, Option<u32>)> {
        list.iter()
            .map(|f| {
                (
                    f.channel.login.as_str(),
                    f.live.as_ref().map(|s| s.stats.viewers),
                )
            })
            .collect()
    }

    #[test]
    fn a_stream_that_ended_joins_the_rest_and_a_new_one_comes_up() {
        let list = [
            followed("big", Some(900)),
            followed("ended", Some(10)),
            followed("zeta", None),
            followed("alpha", None),
        ];
        let live = [followed("big", Some(950)), followed("zeta", Some(40))];
        assert_eq!(
            shown(&with_live(&list, &live)),
            [
                ("big", Some(950)),
                ("zeta", Some(40)),
                ("alpha", None),
                ("ended", None)
            ]
        );
        // Followed since the list was asked for, and streaming already.
        let live = [followed("new", Some(3))];
        assert_eq!(shown(&with_live(&list, &live))[0], ("new", Some(3)),);
    }

    #[test]
    fn a_stream_is_its_category_and_title() {
        let text = crate::lang::Language::English.strings();
        let mut stream = followed("x", Some(1)).live.unwrap();
        stream.category = "Art".into();
        stream.title = "Cozy".into();
        assert_eq!(about(&stream, text), "Art · Cozy");
        stream.category = " ".into();
        assert_eq!(about(&stream, text), "Cozy");
        stream.title = String::new();
        assert_eq!(about(&stream, text), "(no title)");
    }
}
