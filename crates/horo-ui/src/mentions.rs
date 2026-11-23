//! Suggesting people to mention while writing: `@` and the start of a
//! name brings up the room's members who match.

use horo_core::dto::RoomMemberDto;

/// Someone the composer can mention.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub user_id: String,
    /// What goes into the text: their display name, or their username.
    pub name: String,
    pub avatar: Option<String>,
}

impl Person {
    pub fn from_member(member: &RoomMemberDto) -> Person {
        let name = member.display_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| localpart(&member.user_id).to_owned());
        Person { user_id: member.user_id.clone(), name, avatar: member.avatar_url.clone() }
    }
}

pub fn localpart(user_id: &str) -> &str {
    user_id.trim_start_matches('@').split(':').next().unwrap_or(user_id)
}

/// The `@query` being typed at `caret`, if any: the query (possibly
/// empty, right after the `@`) and where the `@` is. The `@` starts a
/// word, so an email address doesn't count.
pub fn typing_mention(text: &str, caret: usize) -> Option<(String, usize)> {
    let before = text.get(..caret)?;
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.contains(|c: char| c.is_whitespace() || c == '@') || query.chars().count() > 32 {
        return None;
    }
    if before[..at].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    Some((query.to_owned(), at))
}

/// Up to `limit` of `people` matching `query`, names that start with it
/// first, then any word of the name or the user ID containing it.
pub fn match_people(people: &[Person], query: &str, limit: usize) -> Vec<Person> {
    let query = query.to_lowercase();
    let mut scored: Vec<(u8, &Person)> = people
        .iter()
        .filter_map(|p| {
            let name = p.name.to_lowercase();
            let id = p.user_id.to_lowercase();
            let score = if name.starts_with(&query) {
                0
            } else if name.split_whitespace().any(|w| w.starts_with(&query)) || id.get(1..).unwrap_or("").starts_with(&query) {
                1
            } else if name.contains(&query) || id.contains(&query) {
                2
            } else {
                return None;
            };
            Some((score, p))
        })
        .collect();
    scored.sort_by(|(a, p), (b, q)| a.cmp(b).then_with(|| p.name.to_lowercase().cmp(&q.name.to_lowercase())));
    scored.into_iter().take(limit).map(|(_, p)| p.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, name: &str) -> Person {
        Person { user_id: id.into(), name: name.into(), avatar: None }
    }

    #[test]
    fn an_at_starting_a_word_is_a_mention() {
        assert_eq!(typing_mention("hi @bo", 6), Some(("bo".into(), 3)));
        assert_eq!(typing_mention("@", 1), Some((String::new(), 0)));
        assert_eq!(typing_mention("me@mail", 7), None);
        assert_eq!(typing_mention("@bob is here", 12), None);
    }

    #[test]
    fn names_starting_with_it_come_first() {
        let people = [person("@zed:x", "Zed Bobson"), person("@bob:x", "Bob"), person("@ann:x", "Ann"), person("@q:x", "jabob")];
        let names: Vec<String> = match_people(&people, "bo", 8).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Bob", "Zed Bobson", "jabob"]);
        assert_eq!(match_people(&people, "", 2).len(), 2);
    }
}
