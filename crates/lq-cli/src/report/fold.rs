//! Groups repeats of a problem: the same check, severity and message.

use lq_check::Offense;
use std::collections::HashMap;

pub struct Group<'a> {
    pub first: &'a Offense,
    pub rest: Vec<&'a Offense>,
}

/// In the order given, with each group where its first offense is.
pub fn fold<'a>(offenses: &[&'a Offense]) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    let mut index: HashMap<(&str, u8, &str), usize> = HashMap::new();
    for &o in offenses {
        let key = (o.check, o.severity as u8, o.message.as_str());
        match index.get(&key) {
            Some(&i) => groups[i].rest.push(o),
            None => {
                index.insert(key, groups.len());
                groups.push(Group { first: o, rest: Vec::new() });
            }
        }
    }
    groups
}

pub fn unfolded<'a>(offenses: &[&'a Offense]) -> Vec<Group<'a>> {
    offenses.iter().map(|&first| Group { first, rest: Vec::new() }).collect()
}
