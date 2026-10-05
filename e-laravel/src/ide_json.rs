//! Package-declared completions: Laravel Idea's `ide.json`.
//!
//! Packages (and projects) can ship an `ide.json` that says "the first argument
//! of `Axis::__construct` is one of these strings" or "the second argument of
//! `->rule()` on `\Pkg\Validation` is a validation rule". It is the format the
//! Laravel ecosystem already writes for PhpStorm; reading it gives `e` the same
//! knowledge for free, one file per package, instead of one feature per package.
//!
//! Supported here: the `completions` section — `complete` kinds `e` has data
//! for, bound by `condition`s on function / method / constructor name, parameter
//! position, and `place` (a plain parameter or inside an array literal). The
//! parsers and the call-site detection are pure and tested.

use std::path::Path;

use lsp_types::{CompletionItem, CompletionItemKind};
use serde_json::Value;

use crate::laravel::{self, Helper, LaravelData};

/// What a rule completes. Kinds `e` has no data for parse to `Unsupported` and
/// complete nothing, so an unknown kind never breaks a file's other rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    RouteName,
    ViewName,
    ConfigKey,
    TranslationKey,
    EnvironmentVariable,
    BladeComponent,
    ValidationRule,
    StaticStrings,
    Gate,
    InertiaPage,
    Unsupported(String),
}

impl Kind {
    fn parse(s: &str) -> Self {
        match s {
            "routeName" => Self::RouteName,
            "viewName" => Self::ViewName,
            "configKey" => Self::ConfigKey,
            "translationKey" => Self::TranslationKey,
            "environmentVariable" => Self::EnvironmentVariable,
            "bladeComponent" => Self::BladeComponent,
            "validationRule" | "validationRules" => Self::ValidationRule,
            "staticStrings" => Self::StaticStrings,
            "gate" | "policy" | "authRule" => Self::Gate,
            "inertiaPage" => Self::InertiaPage,
            other => Self::Unsupported(other.to_string()),
        }
    }
}

/// Where a completion applies. Class names are short (the last segment of an
/// FQN): `e` doesn't infer receiver types, so `->method()` matches on the
/// method name and `Class::method()` / `new Class()` on the class's short name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Condition {
    pub function_names: Vec<String>,
    pub method_names: Vec<String>,
    pub class_names: Vec<String>,
    /// The condition is about `new Class(...)`.
    pub new_class: bool,
    /// 1-based parameter positions; empty means any.
    pub parameters: Vec<usize>,
    /// `place` is an array position (`arrayValue`, `arrayKey`, …) rather than
    /// the parameter itself.
    pub array: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: Kind,
    /// Any one matching condition applies the rule.
    pub conditions: Vec<Condition>,
    /// For `staticStrings`: the strings.
    pub strings: Vec<String>,
}

fn short(name: &str) -> String {
    name.trim_start_matches('\\')
        .rsplit('\\')
        .next()
        .unwrap_or(name)
        .to_string()
}

fn strings_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_condition(c: &Value) -> Condition {
    let mut function_names: Vec<String> = strings_of(c, "functionNames");
    function_names.extend(strings_of(c, "functionFqn").iter().map(|f| short(f)));
    let mut class_names: Vec<String> = Vec::new();
    for key in ["classFqn", "classNames", "classParentFqn"] {
        class_names.extend(strings_of(c, key).iter().map(|f| short(f)));
    }
    let mut new_class = false;
    for key in ["newClassFqn", "newClassNames", "newClassParentFqn"] {
        let v = strings_of(c, key);
        if !v.is_empty() {
            new_class = true;
            class_names.extend(v.iter().map(|f| short(f)));
        }
    }
    let parameters = c
        .get("parameters")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .map(|n| n as usize)
                .collect()
        })
        .unwrap_or_default();
    let array = matches!(
        c.get("place").and_then(Value::as_str),
        Some("arrayValue") | Some("arrayKey") | Some("arrayValueWithKey")
    );
    Condition {
        function_names,
        method_names: strings_of(c, "methodNames"),
        class_names,
        new_class,
        parameters,
        array,
    }
}

/// The rules in one `ide.json` document.
pub fn parse(json: &Value) -> Vec<Rule> {
    json.get("completions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let kind = Kind::parse(r.get("complete")?.as_str()?);
                    let conditions: Vec<Condition> = r
                        .get("condition")
                        .and_then(Value::as_array)
                        .map(|cs| cs.iter().map(parse_condition).collect())
                        .unwrap_or_default();
                    if conditions.is_empty() {
                        return None;
                    }
                    let strings = r
                        .get("options")
                        .map(|o| strings_of(o, "strings"))
                        .unwrap_or_default();
                    Some(Rule {
                        kind,
                        conditions,
                        strings,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every rule the project and its installed packages declare: `<root>/ide.json`
/// and `vendor/<vendor>/<package>/ide.json`.
pub fn load(root: &Path) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut read = |p: &Path| {
        if let Ok(text) = std::fs::read_to_string(p) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                rules.extend(parse(&v));
            }
        }
    };
    read(&root.join("ide.json"));
    if let Ok(vendors) = std::fs::read_dir(root.join("vendor")) {
        for vendor in vendors.flatten().take(500) {
            let Ok(packages) = std::fs::read_dir(vendor.path()) else {
                continue;
            };
            for package in packages.flatten().take(500) {
                let p = package.path().join("ide.json");
                if p.is_file() {
                    read(&p);
                }
            }
        }
    }
    rules
}

// ---- Where the cursor is ----------------------------------------------------------

/// The cursor is inside a string literal that is an argument of a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite<'a> {
    /// Function or method name, or the class for `new Class(`.
    pub callee: &'a str,
    /// The class before `::`, when there is one.
    pub class: Option<&'a str>,
    pub is_method: bool,
    pub is_new: bool,
    /// 1-based position of the argument the string is in.
    pub parameter: usize,
    /// The string sits inside an array literal within that argument.
    pub in_array: bool,
    /// What has been typed of the string so far.
    pub prefix: &'a str,
}

/// Parse the line up to the cursor. Single-line calls only, which is where
/// completion is asked for; anything with an unbalanced or unclear shape is
/// `None` rather than a guess.
pub fn call_site(line_before: &str) -> Option<CallSite<'_>> {
    // The string we're in: the last quote, which must be an opening one — an
    // even number of quotes precede it.
    let qpos = line_before.rfind(['\'', '"'])?;
    let prefix = &line_before[qpos + 1..];
    if prefix.contains(['\'', '"']) {
        return None;
    }
    let quotes_before = line_before[..qpos]
        .chars()
        .filter(|c| *c == '\'' || *c == '"')
        .count();
    if quotes_before % 2 == 1 {
        return None;
    }

    // Walk back to the call's `(`, counting commas at our nesting level.
    let bytes = line_before.as_bytes();
    let mut i = qpos;
    let mut depth_stack: Vec<u8> = Vec::new();
    let mut commas = 0usize;
    let mut in_array = false;
    let mut in_string: Option<u8> = None;
    let mut paren = None;
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        if let Some(q) = in_string {
            if b == q {
                in_string = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => in_string = Some(b),
            b')' | b']' => depth_stack.push(b),
            b'(' | b'[' => {
                if depth_stack.pop().is_none() {
                    if b == b'[' {
                        // An unclosed `[`: we're inside an array literal; the
                        // commas we counted were the array's, not the call's.
                        in_array = true;
                        commas = 0;
                    } else {
                        paren = Some(i);
                        break;
                    }
                }
            }
            b',' if depth_stack.is_empty() => commas += 1,
            _ => {}
        }
    }
    let paren = paren?;

    // The callee before the paren.
    let head = line_before[..paren].trim_end();
    let name_start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| i)?;
    let callee = &head[name_start..];
    if callee.is_empty() {
        return None;
    }
    let before = head[..name_start].trim_end();
    let (class, is_method, is_new) = if let Some(cls) = before.strip_suffix("::") {
        let cls_start = cls
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '\\')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(cls.len());
        let cls = cls[cls_start..].rsplit('\\').next().unwrap_or("");
        (Some(cls).filter(|c| !c.is_empty()), true, false)
    } else if before.ends_with("->") || before.ends_with("?->") {
        (None, true, false)
    } else if before.ends_with("new") || before.ends_with("new \\") {
        (Some(callee), false, true)
    } else {
        (None, false, false)
    };
    Some(CallSite {
        callee,
        class,
        is_method,
        is_new,
        parameter: commas + 1,
        in_array,
        prefix,
    })
}

fn condition_matches(cond: &Condition, site: &CallSite<'_>) -> bool {
    let name_ok = if site.is_new {
        cond.new_class && cond.class_names.iter().any(|c| c == site.callee)
    } else if site.is_method {
        cond.method_names.iter().any(|m| m == site.callee)
            && (cond.class_names.is_empty()
                || site
                    .class
                    .map(|c| cond.class_names.iter().any(|n| n == c))
                    .unwrap_or(true))
    } else {
        cond.function_names.iter().any(|f| f == site.callee)
    };
    let param_ok = cond.parameters.is_empty() || cond.parameters.contains(&site.parameter);
    name_ok && param_ok && cond.array == site.in_array
}

/// The rules that apply at `site`.
pub fn matching<'r>(rules: &'r [Rule], site: &CallSite<'_>) -> Vec<&'r Rule> {
    rules
        .iter()
        .filter(|r| r.conditions.iter().any(|c| condition_matches(c, site)))
        .collect()
}

/// Completion items for whatever the matching rules say the string is.
pub fn complete(
    rules: &[Rule],
    site: &CallSite<'_>,
    data: &LaravelData,
    root: &Path,
) -> Vec<CompletionItem> {
    let lower = site.prefix.to_lowercase();
    let keep = |s: &str| lower.is_empty() || s.to_lowercase().contains(&lower);
    let plain = |label: &str, detail: &str| CompletionItem {
        label: label.to_string(),
        insert_text: Some(label.to_string()),
        kind: Some(CompletionItemKind::VALUE),
        detail: Some(detail.to_string()),
        ..Default::default()
    };
    let mut out = Vec::new();
    for rule in matching(rules, site) {
        match &rule.kind {
            Kind::RouteName => out.extend(laravel::completions(data, Helper::Route, site.prefix)),
            Kind::ViewName => out.extend(laravel::completions(data, Helper::View, site.prefix)),
            Kind::ConfigKey => out.extend(laravel::completions(data, Helper::Config, site.prefix)),
            Kind::TranslationKey => {
                out.extend(laravel::completions(data, Helper::Trans, site.prefix))
            }
            Kind::EnvironmentVariable => {
                out.extend(laravel::completions(data, Helper::Env, site.prefix))
            }
            Kind::BladeComponent => {
                out.extend(laravel::completions(data, Helper::Component, site.prefix))
            }
            Kind::ValidationRule => out.extend(
                crate::validation::rule_names(site.prefix)
                    .into_iter()
                    .map(|r| plain(r, "validation rule")),
            ),
            Kind::StaticStrings => out.extend(
                rule.strings
                    .iter()
                    .filter(|s| keep(s))
                    .map(|s| plain(s, "ide.json")),
            ),
            Kind::Gate => out.extend(
                crate::policies::abilities(root)
                    .into_iter()
                    .filter(|(n, _, _)| keep(n))
                    .map(|(n, _, _)| plain(&n, "ability")),
            ),
            Kind::InertiaPage => out.extend(
                crate::inertia::list_pages(root)
                    .into_iter()
                    .filter(|p| keep(p))
                    .map(|p| plain(&p, "Inertia page")),
            ),
            Kind::Unsupported(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules() -> Vec<Rule> {
        parse(&json!({ "completions": [
            { "complete": "staticStrings",
              "options": { "strings": ["longitude", "latitude"] },
              "condition": [ { "newClassNames": ["Axis"], "parameters": [1] } ] },
            { "complete": "validationRule",
              "condition": [ { "methodNames": ["rule"], "parameters": [1],
                               "classFqn": ["\\\\PackageNamespace\\\\Validation"] } ] },
            { "complete": "routeName",
              "condition": [ { "functionNames": ["redirect_to"], "parameters": [1] } ] },
            { "complete": "viewName",
              "condition": [ { "functionNames": ["validate_view"], "parameters": [1], "place": "arrayValue" } ] },
            { "complete": "somethingFromTheFuture",
              "condition": [ { "functionNames": ["x"] } ] }
        ]}))
    }

    #[test]
    fn parses_the_documented_shapes() {
        let r = rules();
        assert_eq!(r.len(), 5);
        assert_eq!(r[0].kind, Kind::StaticStrings);
        assert_eq!(r[0].strings, vec!["longitude", "latitude"]);
        assert!(r[0].conditions[0].new_class);
        assert_eq!(r[0].conditions[0].class_names, vec!["Axis"]);
        assert_eq!(r[1].conditions[0].class_names, vec!["Validation"]);
        assert_eq!(r[1].conditions[0].method_names, vec!["rule"]);
        assert!(r[3].conditions[0].array);
        assert_eq!(
            r[4].kind,
            Kind::Unsupported("somethingFromTheFuture".into())
        );
    }

    #[test]
    fn finds_the_call_around_the_cursor() {
        let s = call_site("$a = new Axis('lon").unwrap();
        assert!(s.is_new);
        assert_eq!((s.callee, s.parameter, s.prefix), ("Axis", 1, "lon"));

        let s = call_site("  $q->where('id', 5)->rule('req").unwrap();
        assert!(s.is_method && !s.is_new);
        assert_eq!((s.callee, s.class, s.parameter), ("rule", None, 1));

        let s = call_site("Validation::rule('name', 'max").unwrap();
        assert_eq!(
            (s.callee, s.class, s.parameter),
            ("rule", Some("Validation"), 2)
        );

        let s = call_site("return redirect_to('users.").unwrap();
        assert!(!s.is_method);
        assert_eq!(
            (s.callee, s.parameter, s.prefix),
            ("redirect_to", 1, "users.")
        );

        // Inside an array literal: the array's commas don't count as parameters.
        let s = call_site("$r->validate(['name' => 'required', 'email' => 'em").unwrap();
        assert!(s.in_array);
        assert_eq!((s.callee, s.parameter, s.prefix), ("validate", 1, "em"));

        // Not in a string, or in a closed one: nothing.
        assert!(call_site("foo('a')").is_none());
        assert!(call_site("foo(bar").is_none());
    }

    #[test]
    fn matches_rules_and_completes_from_them() {
        let r = rules();
        let data = LaravelData::default();
        let root = Path::new("/nonexistent");

        let site = call_site("new Axis('l").unwrap();
        let labels: Vec<String> = complete(&r, &site, &data, root)
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert_eq!(labels, vec!["longitude", "latitude"]);
        // Second parameter: the rule says only the first.
        assert!(matching(&r, &call_site("new Axis('x', 'l").unwrap()).is_empty());

        // Validation rules on `rule()` — on the declared class, or an unknown receiver.
        let site = call_site("Validation::rule('req").unwrap();
        assert!(complete(&r, &site, &data, root)
            .iter()
            .any(|i| i.label == "required"));
        assert!(matching(&r, &call_site("Other::rule('req").unwrap()).is_empty());
        assert!(!matching(&r, &call_site("$v->rule('req").unwrap()).is_empty());

        // Place must match: viewName only inside the array.
        assert!(matching(&r, &call_site("validate_view('x").unwrap()).is_empty());
        assert!(!matching(&r, &call_site("validate_view(['x").unwrap()).is_empty());
    }
}
