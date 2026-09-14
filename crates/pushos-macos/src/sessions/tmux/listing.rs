//! Reading what tmux prints about its sessions.
//!
//! PushOS asks tmux for its listings in a format of its own choosing, with
//! fields separated by control characters no title or device can contain, so
//! reading them back needs no guessing.

use super::ROWS;

/// Separates fields on one line of a listing.
pub(super) const FIELD: char = '\u{1f}';

/// Begins each pane's screen in a batch of them.
pub(super) const RECORD: char = '\u{1e}';

/// One pane, as tmux described it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::sessions) struct Pane {
    /// tmux's own identity for it, such as `%3`.
    pub(in crate::sessions) id: String,
    /// The terminal device the program in it runs on.
    pub(in crate::sessions) device: String,
    /// The session it belongs to.
    pub(in crate::sessions) session: String,
    /// What the program in it calls itself, or nothing if it has not said.
    pub(in crate::sessions) title: String,
    /// The bottom of what is on it.
    pub(in crate::sessions) screen: String,
}

/// A window showing a tmux session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::sessions) struct Client {
    /// The terminal device of the window it is in.
    pub(in crate::sessions) device: String,
    /// The session it is showing.
    pub(in crate::sessions) session: String,
}

/// Everything tmux is keeping, and every window showing any of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::sessions) struct Layout {
    pub(in crate::sessions) panes: Vec<Pane>,
    pub(in crate::sessions) clients: Vec<Client>,
}

/// Reads the listing of clients and panes, each pane with its height.
pub(super) fn parse_listing(text: &str) -> (Vec<Client>, Vec<(Pane, u32)>) {
    let mut clients = Vec::new();
    let mut panes = Vec::new();

    for line in text.lines() {
        let mut fields = line.split(FIELD);
        match fields.next() {
            Some("C") => {
                if let (Some(device), Some(session)) = (fields.next(), fields.next())
                    && !device.is_empty()
                {
                    clients.push(Client {
                        device: device.to_owned(),
                        session: session.to_owned(),
                    });
                }
            }
            Some("P") => {
                let parts: Vec<&str> = fields.collect();
                let [id, device, session, height, dead, host, title @ ..] = parts.as_slice() else {
                    continue;
                };
                // A pane whose program has exited and that tmux was told to
                // keep is a record of something finished, not a session.
                if *dead == "1" {
                    continue;
                }
                let title = title.join(&FIELD.to_string());
                // tmux titles a pane with the machine's name until the program
                // in it says otherwise, which tells an operator nothing.
                let title = if title == *host { String::new() } else { title };
                panes.push((
                    Pane {
                        id: (*id).to_owned(),
                        device: (*device).to_owned(),
                        session: (*session).to_owned(),
                        title,
                        screen: String::new(),
                    },
                    height.parse().unwrap_or(ROWS),
                ));
            }
            _ => {}
        }
    }
    (clients, panes)
}

/// Splits a batch of screens into each pane's own.
pub(super) fn parse_screens(text: &str) -> Vec<(String, String)> {
    text.split(RECORD)
        .filter_map(|chunk| {
            let (id, screen) = chunk.split_once('\n').unwrap_or((chunk, ""));
            let id = id.trim();
            id.starts_with('%')
                .then(|| (id.to_owned(), screen.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(lines: &[&[&str]]) -> String {
        lines
            .iter()
            .map(|fields| fields.join(&FIELD.to_string()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_listing_becomes_the_panes_and_the_windows_showing_them() {
        let text = listing(&[
            &["C", "/dev/ttys002", "client-1"],
            &[
                "P",
                "%0",
                "/dev/ttys006",
                "client-1",
                "30",
                "0",
                "mac.local",
                "✳ Fix login",
            ],
            &[
                "P",
                "%1",
                "/dev/ttys007",
                "client-2",
                "30",
                "0",
                "mac.local",
                "mac.local",
            ],
            &[
                "P",
                "%2",
                "/dev/ttys008",
                "finished",
                "30",
                "1",
                "mac.local",
                "done",
            ],
        ]);
        let (clients, panes) = parse_listing(&text);

        assert_eq!(
            clients,
            [Client {
                device: "/dev/ttys002".to_owned(),
                session: "client-1".to_owned()
            }]
        );
        assert_eq!(panes.len(), 2, "a dead pane is not a session");
        assert_eq!(panes[0].0.title, "✳ Fix login");
        assert_eq!(
            panes[1].0.title, "",
            "the machine's name is not something the program said"
        );
        assert_eq!(panes[0].1, 30);
    }

    #[test]
    fn a_title_containing_the_separator_is_still_one_title() {
        let odd = format!("a{FIELD}b");
        let text = listing(&[&["P", "%0", "/dev/ttys006", "s", "30", "0", "mac", &odd]]);
        let (_, panes) = parse_listing(&text);
        assert_eq!(panes[0].0.title, odd);
    }

    #[test]
    fn a_batch_of_screens_is_split_by_pane() {
        let text = format!("{RECORD}%0\n❯ \n{RECORD}%1\nline one\nline two\n");
        let screens = parse_screens(&text);
        assert_eq!(
            screens,
            [
                ("%0".to_owned(), "❯ \n".to_owned()),
                ("%1".to_owned(), "line one\nline two\n".to_owned()),
            ]
        );
    }
}
