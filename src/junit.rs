//! JUnit XML rendering of a probe report, for CI systems that display test
//! results from JUnit files. One suite holds the manifest cases; a second
//! holds readiness: whether it was measured, each declared floor, and every
//! lost point as output. Built from the share-safe report only, with no
//! timestamps or durations, so the document is deterministic.

use crate::probe::ProbeReport;

struct Case {
    classname: String,
    name: String,
    /// `failure` or `error`, its type label, and its message and body.
    outcome: Option<(&'static str, String, String, String)>,
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            ch if (ch as u32) < 0x20 && !matches!(ch, '\n' | '\r' | '\t') => {}
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn suite(
    out: &mut String,
    name: &str,
    cases: &[Case],
    properties: &[(String, String)],
    output: &str,
) {
    let failures = cases
        .iter()
        .filter(|case| matches!(case.outcome, Some(("failure", ..))))
        .count();
    let errors = cases
        .iter()
        .filter(|case| matches!(case.outcome, Some(("error", ..))))
        .count();
    out.push_str(&format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"0\">\n",
        escape(name),
        cases.len()
    ));
    if !properties.is_empty() {
        out.push_str("    <properties>\n");
        for (key, value) in properties {
            out.push_str(&format!(
                "      <property name=\"{}\" value=\"{}\"/>\n",
                escape(key),
                escape(value)
            ));
        }
        out.push_str("    </properties>\n");
    }
    for case in cases {
        let open = format!(
            "    <testcase classname=\"{}\" name=\"{}\"",
            escape(&case.classname),
            escape(&case.name)
        );
        match &case.outcome {
            None => out.push_str(&format!("{open}/>\n")),
            Some((element, kind, message, body)) => out.push_str(&format!(
                "{open}>\n      <{element} type=\"{}\" message=\"{}\">{}</{element}>\n    </testcase>\n",
                escape(kind),
                escape(message),
                escape(body)
            )),
        }
    }
    if !output.is_empty() {
        out.push_str(&format!(
            "    <system-out>{}</system-out>\n",
            escape(output)
        ));
    }
    out.push_str("  </testsuite>\n");
}

/// Render the report as a JUnit XML document.
pub fn render_junit(server: &str, report: &ProbeReport) -> String {
    let manifest: Vec<Case> = report
        .cases
        .iter()
        .map(|case| Case {
            classname: format!("{server}.{}", case.probe.as_str()),
            name: case.id.clone(),
            outcome: case.reason.map(|reason| {
                let element = if case.errored() { "error" } else { "failure" };
                (
                    element,
                    reason.as_str().to_owned(),
                    reason.as_str().to_owned(),
                    case.hint().unwrap_or_default(),
                )
            }),
        })
        .collect();

    let classname = format!("{server}.readiness");
    let mut readiness = Vec::new();
    let mut properties = Vec::new();
    let mut output = String::new();
    match (&report.readiness, report.readiness_error) {
        (Some(measured), _) => {
            properties.push(("standard".into(), measured.standard.clone()));
            properties.push(("score".into(), measured.score.to_string()));
            if let Some(score) = measured.score_without_latency {
                properties.push(("score_without_latency".into(), score.to_string()));
            }
            for area in &measured.areas {
                properties.push((
                    format!("area.{}", area.area.as_str()),
                    area.score.to_string(),
                ));
            }
            readiness.push(Case {
                classname: classname.clone(),
                name: format!("readiness {}/100", measured.score),
                outcome: None,
            });
            for check in measured.areas.iter().flat_map(|area| &area.checks) {
                output.push_str(&format!(
                    "lost {}{} score={} reason={}: {}\n",
                    check.id.as_str(),
                    check
                        .tool
                        .as_deref()
                        .map(|tool| format!(" tool={tool}"))
                        .unwrap_or_default(),
                    check.score,
                    check.reason.as_str(),
                    crate::remediation::check_hint(check.reason)
                ));
            }
        }
        (None, Some(reason)) => readiness.push(Case {
            classname: classname.clone(),
            name: "readiness".into(),
            outcome: Some((
                "error",
                reason.as_str().to_owned(),
                format!("readiness not measured: {}", reason.as_str()),
                crate::remediation::hint(reason).to_owned(),
            )),
        }),
        (None, None) => {}
    }
    if let Some(gate) = &report.readiness_gate {
        let latency = if gate.floors.exclude_latency {
            " (latency excluded)"
        } else {
            ""
        };
        let floors = gate
            .floors
            .min_score
            .map(|floor| ("score".to_owned(), floor))
            .into_iter()
            .chain(
                gate.floors
                    .min_areas
                    .iter()
                    .map(|(area, floor)| (area.as_str().to_owned(), *floor)),
            );
        for (subject, floor) in floors {
            let outcome = match &gate.shortfalls {
                None => Some((
                    "error",
                    "readiness-unmeasured".to_owned(),
                    "readiness was not measured".to_owned(),
                    String::new(),
                )),
                Some(shortfalls) => shortfalls
                    .iter()
                    .find(|shortfall| shortfall.subject == subject)
                    .map(|shortfall| {
                        (
                            "failure",
                            "readiness-floor".to_owned(),
                            format!("{subject} {} < {floor}{latency}", shortfall.observed),
                            String::new(),
                        )
                    }),
            };
            readiness.push(Case {
                classname: classname.clone(),
                name: format!("floor {subject} >= {floor}{latency}"),
                outcome,
            });
        }
    }

    let all = manifest.iter().chain(&readiness);
    let tests = manifest.len() + readiness.len();
    let failures = all
        .clone()
        .filter(|case| matches!(case.outcome, Some(("failure", ..))))
        .count();
    let errors = all
        .filter(|case| matches!(case.outcome, Some(("error", ..))))
        .count();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites name=\"mcpeval\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\">\n"
    ));
    if report.gate().is_some() {
        suite(&mut out, &format!("{server}.manifest"), &manifest, &[], "");
    }
    if !readiness.is_empty() {
        suite(&mut out, &classname, &readiness, &properties, &output);
    }
    out.push_str("</testsuites>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_and_control_characters_are_escaped() {
        assert_eq!(
            escape("a<b & \"c\" 'd'>\u{1}\n"),
            "a&lt;b &amp; &quot;c&quot; &apos;d&apos;&gt;\n"
        );
    }

    #[test]
    fn an_empty_report_is_an_empty_document() {
        assert_eq!(
            render_junit("demo", &ProbeReport::default()),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"mcpeval\" tests=\"0\" failures=\"0\" errors=\"0\">\n</testsuites>\n"
        );
    }
}
