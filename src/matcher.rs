//! 手写的模糊匹配打分（Sublime / fzf 那一类），不依赖任何库。
//!
//! 匹配规则是**子序列**：`gve` 能匹配 `give`，不要求连续。打分用一趟动态规划：
//!
//! ```text
//! score[i][j] = 在前 j 个字符里匹配 needle[..i]、且 needle[i-1] 落在 haystack[j-1]
//!               时的最高分
//! score[i][j] = max( score[i-1][j-1] + 连击奖励,      // 紧挨着上一个命中
//!                    max_{k<j-1} score[i-1][k] + 间隔罚分 * (j-1-k) )
//!               + 单字符得分 + 位置奖励
//! ```
//!
//! 位置奖励让"开头 / 分隔符之后 / 驼峰边界"上的命中排在前面：对 `pi` 来说
//! `player_id` 比 `inventory_players` 更像用户想要的东西；间隔罚分让连续命中排在
//! 前面。DP 是 O(needle × haystack)，候选值都是短字符串，随便算。
//!
//! 回溯路径就是每个命中字符的下标，界面拿它做高亮。

/// 一次匹配的结果。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Match {
    /// 分数，越高越靠前。
    pub score: i32,
    /// 命中的字符下标（按 `char` 计，不是字节）。
    pub indices: Vec<usize>,
}

/// 一个字符对上的基础分。
const MATCH_SCORE: i32 = 16;
/// 每跳过一个字符扣的分。
const GAP_PENALTY: i32 = -3;
/// 命中在字符串开头。
const START_BONUS: i32 = 24;
/// 命中紧跟在分隔符（`_` `-` `.` `:` `/` 空格……）之后。
const BOUNDARY_BONUS: i32 = 16;
/// 命中在驼峰边界上（`playerId` 的 `I`）。
const CAMEL_BONUS: i32 = 12;
/// 紧挨着上一个命中。
const CONSECUTIVE_BONUS: i32 = 10;
/// 哨兵："这条路走不通"。取 `i32::MIN` 的四分之一，做加减都不会溢出。
const DEAD: i32 = i32::MIN / 4;

/// 小写化一个字符。`to_lowercase` 在极少数情况下会展开成多个字符（`İ`），
/// 这里只取第一个，足够用。
fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// 分隔符：它后面的字符算一个"词的开头"。
fn is_separator(c: char) -> bool {
    matches!(c, '_' | '-' | '.' | ':' | '/' | '\\' | ' ' | '|')
}

/// 一个命中位置的奖励：开头 > 分隔符后 > 驼峰边界 > 其它。
fn position_bonus(original: &[char], index: usize) -> i32 {
    if index == 0 {
        return START_BONUS;
    }
    let previous = original[index - 1];
    let current = original[index];
    if is_separator(previous) {
        BOUNDARY_BONUS
    } else if current.is_uppercase() && previous.is_lowercase() {
        CAMEL_BONUS
    } else {
        0
    }
}

/// 子序列匹配 + 打分。
///
/// `needle` 为空时返回 0 分、无命中的 `Match`；匹配不上返回 `None`。
/// `case_sensitive` 为 false（默认）时按 Unicode 小写比较，但位置奖励仍然看原始
/// 大小写（所以驼峰边界仍然认得出来）。
pub fn fuzzy_match(needle: &str, haystack: &str, case_sensitive: bool) -> Option<Match> {
    let needle: Vec<char> = if case_sensitive {
        needle.chars().collect()
    } else {
        needle.chars().map(lower).collect()
    };
    let original: Vec<char> = haystack.chars().collect();
    let haystack: Vec<char> = if case_sensitive {
        original.clone()
    } else {
        original.iter().copied().map(lower).collect()
    };

    if needle.is_empty() {
        return Some(Match::default());
    }
    if needle.len() > haystack.len() {
        return None;
    }

    let width = haystack.len();
    // 第 0 行：needle 的第一个字符能落在哪些位置。
    let mut row = vec![DEAD; width];
    for (index, character) in haystack.iter().enumerate() {
        if *character == needle[0] {
            row[index] = MATCH_SCORE + position_bonus(&original, index);
        }
    }
    // `preds[i][j]`：第 i 个 needle 字符落在 j 时，第 i-1 个字符落在哪。
    // 第 0 行没有前驱。
    let mut predecessors: Vec<Vec<usize>> = vec![vec![usize::MAX; width]];

    for needle_character in needle.iter().skip(1) {
        let previous_row = row.clone();
        let mut current = vec![DEAD; width];
        let mut predecessor = vec![usize::MAX; width];
        // `best_prefix` = max over k <= j-2 of (previous_row[k] - GAP_PENALTY * k)，
        // 这样"从任意更早的位置跳过来"这一项可以 O(1) 算出来。
        let mut best_prefix = DEAD;
        let mut best_prefix_at = usize::MAX;

        for j in 0..width {
            if j >= 2 {
                let k = j - 2;
                if previous_row[k] > DEAD {
                    let candidate = previous_row[k] - GAP_PENALTY * (k as i32);
                    if candidate > best_prefix {
                        best_prefix = candidate;
                        best_prefix_at = k;
                    }
                }
            }
            if haystack[j] != *needle_character {
                continue;
            }
            let mut best = DEAD;
            let mut from = usize::MAX;
            if j >= 1 && previous_row[j - 1] > DEAD {
                best = previous_row[j - 1] + CONSECUTIVE_BONUS;
                from = j - 1;
            }
            if best_prefix > DEAD {
                let candidate = best_prefix + GAP_PENALTY * (j as i32 - 1);
                if candidate > best {
                    best = candidate;
                    from = best_prefix_at;
                }
            }
            if best > DEAD {
                current[j] = best + MATCH_SCORE + position_bonus(&original, j);
                predecessor[j] = from;
            }
        }

        if current.iter().all(|score| *score <= DEAD) {
            return None;
        }
        predecessors.push(predecessor);
        row = current;
    }

    // 最佳结尾。
    let mut best_index = usize::MAX;
    let mut best_score = DEAD;
    for (index, score) in row.iter().enumerate() {
        if *score > best_score {
            best_score = *score;
            best_index = index;
        }
    }
    if best_index == usize::MAX {
        return None;
    }

    // 回溯出每个命中字符的位置。
    let mut indices = vec![0usize; needle.len()];
    let mut j = best_index;
    for i in (0..needle.len()).rev() {
        indices[i] = j;
        if i == 0 {
            break;
        }
        j = predecessors[i][j];
        if j == usize::MAX {
            return None;
        }
    }

    Some(Match {
        score: best_score,
        indices,
    })
}

/// 前缀匹配：`haystack` 是否以 `needle` 开头（按 `case_sensitive` 比较）。
///
/// 它总是严格优于任何子序列匹配，所以补全列表把它单独放在最前面
/// （见 [`crate::Completion::candidates`] 的排序）。
pub fn prefix_match(needle: &str, haystack: &str, case_sensitive: bool) -> Option<Match> {
    let needle: Vec<char> = needle.chars().collect();
    let haystack: Vec<char> = haystack.chars().collect();
    if needle.len() > haystack.len() {
        return None;
    }
    let head = &haystack[..needle.len()];
    let starts_with = if case_sensitive {
        head == needle.as_slice()
    } else {
        head.iter()
            .zip(&needle)
            .all(|(a, b)| lower(*a) == lower(*b))
    };
    if !starts_with {
        return None;
    }
    Some(Match {
        score: MATCH_SCORE * needle.len() as i32 + START_BONUS,
        indices: (0..needle.len()).collect(),
    })
}

/// `needle` 是不是 `haystack` 的前缀。
pub fn is_prefix(needle: &str, haystack: &str, case_sensitive: bool) -> bool {
    prefix_match(needle, haystack, case_sensitive).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_matches_everything() {
        let m = fuzzy_match("", "anything", false).unwrap();
        assert_eq!(m.score, 0);
        assert!(m.indices.is_empty());
    }

    #[test]
    fn a_subsequence_matches_and_reports_positions() {
        let m = fuzzy_match("gve", "give", false).unwrap();
        assert_eq!(m.indices, vec![0, 2, 3]);
    }

    #[test]
    fn out_of_order_characters_do_not_match() {
        assert!(fuzzy_match("evg", "give", false).is_none());
        assert!(fuzzy_match("give!", "give", false).is_none());
    }

    #[test]
    fn prefix_hits_beat_scattered_hits() {
        let scattered = fuzzy_match("pi", "shop_items", false).unwrap();
        let boundary = fuzzy_match("pi", "player_id", false).unwrap();
        let prefix = fuzzy_match("pl", "player_id", false).unwrap();
        assert!(
            prefix.score > scattered.score,
            "prefix {} should beat scattered {}",
            prefix.score,
            scattered.score
        );
        assert!(boundary.score > scattered.score);
    }

    #[test]
    fn consecutive_hits_beat_split_ones() {
        let tight = fuzzy_match("pl", "player", false).unwrap();
        let loose = fuzzy_match("pl", "papal", false).unwrap();
        assert!(tight.score > loose.score);
    }

    #[test]
    fn camel_case_boundaries_are_rewarded() {
        let camel = fuzzy_match("pi", "playerId", false).unwrap();
        let plain = fuzzy_match("pi", "playerid", false).unwrap();
        assert!(camel.score > plain.score);
    }

    #[test]
    fn case_is_folded_unless_asked_otherwise() {
        assert!(fuzzy_match("GIVE", "give", false).is_some());
        assert!(fuzzy_match("GIVE", "give", true).is_none());
        assert!(fuzzy_match("give", "give", true).is_some());
    }

    #[test]
    fn unicode_needles_work() {
        let m = fuzzy_match("玩家", "当前玩家列表", false).unwrap();
        assert_eq!(m.indices, vec![2, 3]);
    }

    #[test]
    fn prefix_match_is_strict_about_the_start() {
        assert!(is_prefix("pla", "player_id", false));
        assert!(is_prefix("PLA", "player_id", false));
        assert!(!is_prefix("lay", "player_id", false));
        assert!(!is_prefix("players", "player_id", false));
        assert!(prefix_match("", "anything", false).is_some());
    }
}
