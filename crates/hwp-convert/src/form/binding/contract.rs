//! Closed, source-bound binding-plan data and bounded value constraints (#418).
//! Schema validation handles shape; these checks also enforce relationships between fields.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub type Values = BTreeMap<String, String>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BindingPlan {
    pub version: u32,
    pub source_sha256: String,
    pub structural_sha256: String,
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub repeats: Vec<ProfileRepeat>,
    #[serde(default)]
    pub constraints: Vec<BindingConstraint>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub key: String,
    pub targets: Vec<BindingTarget>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BindingTarget {
    pub occurrence_id: String,
    #[serde(default = "default_copies")]
    pub copies: Vec<usize>,
    pub max_chars: usize,
    pub allow_reflow: bool,
}

fn default_copies() -> Vec<usize> {
    vec![0]
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileRepeat {
    pub table_id: String,
    pub after_paragraph_id: String,
    /// Total count, including the original profile.
    pub copies: usize,
    pub profile_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingConstraint {
    Equal { left: String, right: String },
    Sum { keys: Vec<String>, total: String },
}

fn valid_text_length(value: &str, max: usize) -> bool {
    let count = value.chars().count();
    (1..=max).contains(&count)
}

fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn plan_error(cause: &str) -> String {
    format!(
        "바인딩 계획이 유효하지 않습니다 ({cause}). 원본을 다시 조회하고 계획의 해당 항목을 수정한 뒤 재검토하세요"
    )
}

fn rule_error(index: usize, cause: &str) -> String {
    format!(
        "바인딩 제약 규칙 {index} 검사가 실패했습니다 ({cause}). 해당 규칙의 정수 값과 합계 정의를 확인한 뒤 다시 시도하세요"
    )
}

impl BindingPlan {
    /// Validates closed-plan bounds and target ownership without interpreting literal keys.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(plan_error("version은 1이어야 합니다"));
        }
        if !valid_sha(&self.source_sha256) || !valid_sha(&self.structural_sha256) {
            return Err(plan_error("SHA-256은 소문자 16진수 64자리여야 합니다"));
        }
        if self.bindings.len() > 256 || self.repeats.len() > 16 || self.constraints.len() > 128 {
            return Err(plan_error(
                "bindings/repeats/constraints 항목 상한을 넘었습니다",
            ));
        }
        let mut keys = BTreeSet::new();
        let mut owners = BTreeSet::new();
        for (index, binding) in self.bindings.iter().enumerate() {
            if !valid_text_length(&binding.key, 128) || !keys.insert(binding.key.as_str()) {
                return Err(plan_error(&format!("binding {index}의 키 길이 또는 중복")));
            }
            if !(1..=256).contains(&binding.targets.len()) {
                return Err(plan_error(&format!(
                    "binding {index}의 targets 개수는 1..=256이어야 합니다"
                )));
            }
            for (target_index, target) in binding.targets.iter().enumerate() {
                if !valid_text_length(&target.occurrence_id, 1024)
                    || !(1..=20000).contains(&target.max_chars)
                    || !(1..=16).contains(&target.copies.len())
                {
                    return Err(plan_error(&format!(
                        "binding {index} target {target_index}의 ID/문자 수/복제 개수 범위"
                    )));
                }
                let mut copies = BTreeSet::new();
                for &copy in &target.copies {
                    if copy > 15 || !copies.insert(copy) {
                        return Err(plan_error(&format!(
                            "binding {index} target {target_index}의 복제 인덱스 범위 또는 중복"
                        )));
                    }
                    if !owners.insert((target.occurrence_id.as_str(), copy)) {
                        return Err(plan_error(
                            "동일한 개별 위치와 복제 인덱스의 소유 대상이 중복됩니다",
                        ));
                    }
                }
            }
        }
        for (index, repeat) in self.repeats.iter().enumerate() {
            if !valid_text_length(&repeat.table_id, 1024)
                || !valid_text_length(&repeat.after_paragraph_id, 1024)
                || !(1..=16).contains(&repeat.copies)
                || repeat.profile_ids.len() != repeat.copies
            {
                return Err(plan_error(&format!(
                    "repeat {index}의 ID/전체 개수/profile_ids 개수 범위"
                )));
            }
            let mut profiles = BTreeSet::new();
            for id in &repeat.profile_ids {
                if !valid_text_length(id, 128) || !profiles.insert(id.as_str()) {
                    return Err(plan_error(&format!(
                        "repeat {index}의 profile_ids 길이 또는 중복"
                    )));
                }
            }
        }
        for (index, constraint) in self.constraints.iter().enumerate() {
            let valid_key = |key: &str| valid_text_length(key, 128);
            match constraint {
                BindingConstraint::Equal { left, right } => {
                    if !valid_key(left) || !valid_key(right) {
                        return Err(rule_error(index, "참조 키 길이 범위"));
                    }
                }
                BindingConstraint::Sum { keys, total } => {
                    if !(1..=256).contains(&keys.len())
                        || !keys.iter().all(|key| valid_key(key))
                        || !valid_key(total)
                    {
                        return Err(rule_error(index, "합계 키 개수 또는 참조 키 길이 범위"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks exact value-key membership, literal text bounds and numeric constraints.
    /// Equal means numeric equality, so `1,000` and `1000` compare equal.
    /// Diagnostics expose only rule indices and static causes, never keys or values.
    pub fn validate_values(&self, values: &Values) -> Result<(), String> {
        self.validate()?;
        if values.len() > 256 {
            return Err(plan_error("값 항목 상한 256개를 넘었습니다"));
        }
        if values.len() != self.bindings.len()
            || self
                .bindings
                .iter()
                .any(|binding| !values.contains_key(&binding.key))
        {
            return Err(plan_error("값 키가 누락되었거나 계획에 없는 키가 있습니다"));
        }
        for (index, binding) in self.bindings.iter().enumerate() {
            let value = &values[&binding.key];
            let chars = value.chars().count();
            if chars > 20000
                || binding
                    .targets
                    .iter()
                    .any(|target| chars > target.max_chars)
                || value
                    .chars()
                    .any(|ch| (ch as u32) < 32 && ch != '\n' && ch != '\t')
            {
                return Err(plan_error(&format!(
                    "binding {index}의 값이 문자 상한 또는 허용 텍스트 규칙을 벗어났습니다"
                )));
            }
        }
        for (index, constraint) in self.constraints.iter().enumerate() {
            let number = |key: &str| -> Result<i64, String> {
                let value = values
                    .get(key)
                    .ok_or_else(|| rule_error(index, "참조 값 누락"))?;
                parse_integer(value)
                    .ok_or_else(|| rule_error(index, "허용되지 않는 정수 표기 또는 i64 상한 초과"))
            };
            let equal = match constraint {
                BindingConstraint::Equal { left, right } => number(left)? == number(right)?,
                BindingConstraint::Sum { keys, total } => {
                    let sum = keys.iter().try_fold(0i64, |sum, key| {
                        sum.checked_add(number(key)?)
                            .ok_or_else(|| rule_error(index, "합계 정수 오버플로"))
                    })?;
                    sum == number(total)?
                }
            };
            if !equal {
                return Err(rule_error(index, "정수 동등성 또는 합계 불일치"));
            }
        }
        Ok(())
    }
}

/// Unsigned ASCII digits, optionally comma-grouped in thousands, bounded by i64::MAX.
/// No whitespace, signs, units, fractions or non-ASCII digits are accepted.
fn parse_integer(value: &str) -> Option<i64> {
    if value.is_empty() {
        return None;
    }
    if value.contains(',') {
        let mut groups = value.split(',');
        let first = groups.next()?;
        if !(1..=3).contains(&first.len()) || !first.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if !groups.all(|group| group.len() == 3 && group.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
    } else if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .bytes()
        .filter(|byte| *byte != b',')
        .try_fold(0i64, |number, digit| {
            number.checked_mul(10)?.checked_add(i64::from(digit - b'0'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(id: &str) -> BindingTarget {
        BindingTarget {
            occurrence_id: id.into(),
            copies: vec![0],
            max_chars: 20000,
            allow_reflow: false,
        }
    }
    fn plan() -> BindingPlan {
        BindingPlan {
            version: 1,
            source_sha256: "a".repeat(64),
            structural_sha256: "b".repeat(64),
            bindings: ["left", "right", "total"]
                .into_iter()
                .map(|key| Binding {
                    key: key.into(),
                    targets: vec![target(key)],
                })
                .collect(),
            repeats: vec![],
            constraints: vec![],
        }
    }
    fn values_fixture() -> Values {
        [("left", "1,000"), ("right", "2"), ("total", "1002")]
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect()
    }
    #[test]
    fn integer_grammar_and_i64_boundary_are_explicit() {
        for (text, expected) in [
            ("0", 0),
            ("000", 0),
            ("1000", 1000),
            ("1,000", 1000),
            ("12,345,678", 12345678),
            ("9223372036854775807", i64::MAX),
            ("9,223,372,036,854,775,807", i64::MAX),
        ] {
            assert_eq!(parse_integer(text), Some(expected));
        }
        for text in [
            "",
            "1,00",
            "1234,567",
            ",000",
            "1,000,",
            "1,,000",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1.0",
            "1pt",
            "١",
            "１",
            "9223372036854775808",
        ] {
            assert_eq!(parse_integer(text), None, "{text:?}");
        }
    }
    #[test]
    fn numeric_equal_and_checked_sum_accept_valid_values() {
        let mut plan = plan();
        plan.constraints.push(BindingConstraint::Sum {
            keys: vec!["left".into(), "right".into()],
            total: "total".into(),
        });
        assert!(plan.validate_values(&values_fixture()).is_ok());
        plan.constraints = vec![BindingConstraint::Equal {
            left: "left".into(),
            right: "total".into(),
        }];
        let mut values = values_fixture();
        values.insert("total".into(), "1000".into());
        assert!(plan.validate_values(&values).is_ok());
    }
    #[test]
    fn constraint_failures_do_not_echo_semantic_keys_or_values() {
        let mut plan = plan();
        plan.bindings[0].key = "PRIVATE-KEY".into();
        plan.constraints.push(BindingConstraint::Sum {
            keys: vec!["PRIVATE-KEY".into(), "right".into()],
            total: "total".into(),
        });
        let mut values = values_fixture();
        values.remove("left");
        values.insert("PRIVATE-KEY".into(), "9223372036854775807".into());
        let error = plan.validate_values(&values).unwrap_err();
        assert!(error.contains("규칙 0"));
        assert!(!error.contains("PRIVATE-KEY") && !error.contains("9223372036854775807"));
        values.insert("PRIVATE-KEY".into(), "1".into());
        assert!(
            plan.validate_values(&values)
                .unwrap_err()
                .contains("불일치")
        );
        plan.constraints = vec![BindingConstraint::Equal {
            left: "absent-private-key".into(),
            right: "total".into(),
        }];
        let error = plan.validate_values(&values).unwrap_err();
        assert!(error.contains("누락") && !error.contains("absent-private-key"));
    }
    #[test]
    fn values_are_exact_and_plain_text_is_bounded() {
        let plan = plan();
        let mut values = values_fixture();
        values.remove("left");
        assert!(plan.validate_values(&values).is_err());
        values.insert("extra".into(), "value".into());
        assert!(plan.validate_values(&values).is_err());
        let mut values = values_fixture();
        values.insert("left".into(), "한글\n\tvalue".into());
        assert!(plan.validate_values(&values).is_ok());
        for invalid in ["value\r", "value\0"] {
            values.insert("left".into(), invalid.into());
            assert!(plan.validate_values(&values).is_err());
        }
        values.insert("left".into(), "가".repeat(20001));
        assert!(plan.validate_values(&values).is_err());
    }
    #[test]
    fn serde_contract_is_closed_and_requires_reflow_choice() {
        let value = serde_json::to_value(plan()).unwrap();
        for pointer in ["", "/bindings/0", "/bindings/0/targets/0"] {
            let mut invalid = value.clone();
            invalid.pointer_mut(pointer).unwrap()["unknown"] = json!(true);
            assert!(serde_json::from_value::<BindingPlan>(invalid).is_err());
        }
        let mut missing = value.clone();
        missing["bindings"][0]["targets"][0]
            .as_object_mut()
            .unwrap()
            .remove("allow_reflow");
        assert!(serde_json::from_value::<BindingPlan>(missing).is_err());
        let mut defaulted = value;
        defaulted["bindings"][0]["targets"][0]
            .as_object_mut()
            .unwrap()
            .remove("copies");
        let parsed: BindingPlan = serde_json::from_value(defaulted).unwrap();
        assert_eq!(parsed.bindings[0].targets[0].copies, [0]);
        assert!(
            serde_json::from_value::<BindingConstraint>(
                json!({"kind":"equal","left":"a","right":"b","extra":0})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<ProfileRepeat>(json!({"table_id":"t","after_paragraph_id":"p","copies":1,"profile_ids":["a"],"extra":0})).is_err());
    }
    #[test]
    fn plan_bounds_and_duplicate_target_ownership_refuse() {
        let mut invalid = plan();
        invalid.version = 2;
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.source_sha256 = "A".repeat(64);
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.bindings[1].key = "left".into();
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.bindings[1].targets = invalid.bindings[0].targets.clone();
        assert!(invalid.validate().is_err());
        for copies in [vec![], vec![0, 0], vec![16]] {
            let mut invalid = plan();
            invalid.bindings[0].targets[0].copies = copies;
            assert!(invalid.validate().is_err());
        }
        for max in [0, 20001] {
            let mut invalid = plan();
            invalid.bindings[0].targets[0].max_chars = max;
            assert!(invalid.validate().is_err());
        }
        let mut invalid = plan();
        invalid.bindings = vec![invalid.bindings[0].clone(); 257];
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.bindings[0].targets.clear();
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.constraints = vec![BindingConstraint::Sum {
            keys: vec![],
            total: "total".into(),
        }];
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.repeats.push(ProfileRepeat {
            table_id: "t".into(),
            after_paragraph_id: "p".into(),
            copies: 2,
            profile_ids: vec!["same".into(), "same".into()],
        });
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn each_target_max_chars_is_enforced_as_unicode_scalar_budget() {
        let mut plan = plan();
        plan.bindings[0].targets[0].max_chars = 2;
        plan.bindings[0].targets.push(BindingTarget {
            copies: vec![1],
            ..target("left")
        });
        let mut values = values_fixture();
        values.insert("left".into(), "가나".into());
        assert!(plan.validate_values(&values).is_ok());
        values.insert("left".into(), "가나다".into());
        assert!(plan.validate_values(&values).is_err());
    }
    #[test]
    fn missing_and_invalid_numeric_failures_are_redacted() {
        let mut plan = plan();
        plan.bindings[0].key = "PRIVATE-NUMERIC-KEY".into();
        plan.constraints = vec![BindingConstraint::Equal {
            left: "PRIVATE-NUMERIC-KEY".into(),
            right: "total".into(),
        }];
        let mut values = values_fixture();
        values.remove("left");
        values.insert("PRIVATE-NUMERIC-KEY".into(), "PRIVATE-NUMERIC-VALUE".into());
        let error = plan.validate_values(&values).unwrap_err();
        assert!(error.contains("규칙 0"));
        assert!(!error.contains("PRIVATE-NUMERIC-KEY") && !error.contains("PRIVATE-NUMERIC-VALUE"));
        values.remove("PRIVATE-NUMERIC-KEY");
        let error = plan.validate_values(&values).unwrap_err();
        assert!(error.contains("누락"));
        assert!(!error.contains("PRIVATE-NUMERIC-KEY"));
    }
    #[test]
    fn keys_remain_literal_and_values_map_has_a_fixed_cap() {
        let mut plan = plan();
        plan.bindings[0].key = " left ".into();
        let mut values = values_fixture();
        values.remove("left");
        values.insert(" left ".into(), "literal".into());
        assert!(plan.validate_values(&values).is_ok());
        values.remove(" left ");
        values.insert("left".into(), "literal".into());
        assert!(plan.validate_values(&values).is_err());
        let oversized = (0..257)
            .map(|index| (format!("key-{index}"), String::new()))
            .collect();
        assert!(
            plan.validate_values(&oversized)
                .unwrap_err()
                .contains("256")
        );
    }
    #[test]
    fn repeat_constraint_and_identifier_limits_refuse() {
        let mut invalid = plan();
        invalid.bindings[0].key = String::new();
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.bindings[0].key = "k".repeat(129);
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.bindings[0].targets[0].occurrence_id = "i".repeat(1025);
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.bindings[0].targets = (0..257)
            .map(|index| target(&format!("id-{index}")))
            .collect();
        assert!(invalid.validate().is_err());
        let repeat = ProfileRepeat {
            table_id: "t".into(),
            after_paragraph_id: "p".into(),
            copies: 1,
            profile_ids: vec!["profile".into()],
        };
        for copies in [0, 17] {
            let mut invalid = plan();
            let mut repeat = repeat.clone();
            repeat.copies = copies;
            invalid.repeats.push(repeat);
            assert!(invalid.validate().is_err());
        }
        let mut invalid = plan();
        invalid.repeats = vec![repeat.clone(); 17];
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        let mut long_id = repeat.clone();
        long_id.profile_ids[0] = "p".repeat(129);
        invalid.repeats.push(long_id);
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.constraints = vec![
            BindingConstraint::Equal {
                left: "left".into(),
                right: "right".into()
            };
            129
        ];
        assert!(invalid.validate().is_err());
        let mut invalid = plan();
        invalid.constraints.push(BindingConstraint::Sum {
            keys: vec!["left".into(); 257],
            total: "total".into(),
        });
        assert!(invalid.validate().is_err());
    }
}
