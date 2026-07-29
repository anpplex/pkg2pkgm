//! Mobile wallpaper input compatibility for Scene JSON scripts.
//!
//! Official Android Dino (and other interactive scenes) route `cursorDown` through
//! the `postprocess` layer via `shared.doJump`. Desktop projects often put the
//! full jump body on a character layer (`mario_walk_1.visible.script`). Live
//! wallpapers on Android typically deliver cursor/touch events to the
//! postprocess/fullscreen path; without the relay, applied wallpapers never jump.
//!
//! This pass rewrites non-empty `export function cursorDown` bodies into
//! `function doJump() { … }; shared.doJump = doJump; export function cursorDown(event) {}`
//! and attaches a postprocess `visible.script` relay when missing.

use std::{fs, path::Path};

use serde_json::Value;

use crate::{Error, Result, Stage};

const POSTPROCESS_RELAY_SCRIPT: &str = r#"'use strict';

/**
 * @param {Boolean} value - for property 'visible'
 * @return {Boolean} - update current property value
 */
export function update(value) {

return value;
}

/**
 * @param {ICursorEvent} event
 */
export function cursorDown(event) {
shared.doJump();
}
"#;

const POSTPROCESS_CURSOR_RELAY: &str = r#"/**
 * @param {ICursorEvent} event
 */
export function cursorDown(event) {
shared.doJump();
}
"#;

/// Rewrite snapshot `scene.json` (or declared scene entry) for Android cursor input.
pub fn apply_mobile_scene_input_compat(snapshot_root: &Path, scene_entry: &str) -> Result<()> {
    let path = snapshot_root.join(scene_entry);
    if !path.is_file() {
        return Ok(());
    }
    let original = fs::read(&path).map_err(|source| Error::Io {
        stage: Stage::Pack,
        path: path.clone(),
        source,
    })?;
    let transformed = transform_scene_json_for_mobile_input(&original)?;
    if transformed != original {
        fs::write(&path, transformed).map_err(|source| Error::Io {
            stage: Stage::Pack,
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

/// Pure transform used by tests and [`apply_mobile_scene_input_compat`].
pub fn transform_scene_json_for_mobile_input(scene_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut root: Value =
        serde_json::from_slice(scene_bytes).map_err(|source| Error::InvalidProject {
            reason: format!("scene.json is not valid JSON for mobile input compat: {source}"),
        })?;

    if !mobile_input_rewrite_is_safe(&root) {
        return Ok(scene_bytes.to_vec());
    }

    let mut rewritten_handlers = 0;
    rewrite_scripts_in_value(&mut root, &mut rewritten_handlers);
    if rewritten_handlers != 1 || !ensure_postprocess_cursor_relay(&mut root) {
        // An unambiguous source handler and a safe fullscreen relay target are
        // both required. Otherwise keep the original scene.json bytes
        // (including key order and missing trailing newline) for byte-stable
        // packages and to avoid deleting or competing with existing handlers.
        return Ok(scene_bytes.to_vec());
    }

    // Compact JSON with trailing newline — same style as mobile project.json.
    let mut out = serde_json::to_vec(&root).map_err(|source| Error::InvalidProject {
        reason: format!("failed to serialize scene.json after mobile input compat: {source}"),
    })?;
    if out.last().copied() != Some(b'\n') {
        out.push(b'\n');
    }
    Ok(out)
}

fn mobile_input_rewrite_is_safe(root: &Value) -> bool {
    let Some(postprocess_relay_target) = selected_postprocess_relay_target(root) else {
        return false;
    };
    let mut source_cursor_declarations = 0;
    let mut eligible_source_handlers = 0;
    if !inspect_non_postprocess_scripts(
        root,
        postprocess_relay_target,
        &mut source_cursor_declarations,
        &mut eligible_source_handlers,
    ) || source_cursor_declarations != 1
        || eligible_source_handlers != 1
    {
        return false;
    }

    postprocess_relay_is_safe(root)
}

fn inspect_non_postprocess_scripts(
    value: &Value,
    postprocess_relay_target: &serde_json::Map<String, Value>,
    cursor_declarations: &mut usize,
    eligible_handlers: &mut usize,
) -> bool {
    match value {
        Value::Object(map) => {
            // The selected postprocess `visible.script` is audited separately
            // as the relay append/reuse target. Only that exact property map is
            // excluded here: scripts in nested effects/passes must still pass
            // the same fail-closed source audit.
            if !std::ptr::eq(map, postprocess_relay_target)
                && let Some(Value::String(script)) = map.get("script")
            {
                let Some(analysis) = analyze_script(script) else {
                    return false;
                };
                if analysis_has_executable_identifier(script, &analysis, "eval")
                    || analysis.has_shared_do_jump_owner
                    || analysis.cursor_down_functions.len() > 1
                    || analysis_has_unsafe_computed_member_access(script, &analysis)
                    || (analysis.cursor_down_functions.is_empty()
                        && !non_handler_local_storage_is_safe(script, &analysis))
                {
                    return false;
                }
                if let Some(function) = analysis.cursor_down_functions.first() {
                    if analysis_has_executable_identifier(script, &analysis, "shared")
                        || analysis_has_executable_identifier(script, &analysis, "doJump")
                        || !cursor_handler_is_context_free(script, &analysis, function)
                    {
                        return false;
                    }
                    *cursor_declarations += 1;
                    if !script[function.body_start..function.body_end]
                        .trim()
                        .is_empty()
                    {
                        *eligible_handlers += 1;
                    }
                }
            }
            map.values().all(|child| {
                inspect_non_postprocess_scripts(
                    child,
                    postprocess_relay_target,
                    cursor_declarations,
                    eligible_handlers,
                )
            })
        }
        Value::Array(items) => items.iter().all(|child| {
            inspect_non_postprocess_scripts(
                child,
                postprocess_relay_target,
                cursor_declarations,
                eligible_handlers,
            )
        }),
        _ => true,
    }
}

fn selected_postprocess_relay_target(root: &Value) -> Option<&serde_json::Map<String, Value>> {
    let objects = root.get("objects")?.as_array()?;
    let mut postprocess_objects = objects.iter().filter(|object| {
        object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name == "postprocess")
    });
    let postprocess = postprocess_objects.next()?;
    if postprocess_objects.next().is_some() {
        return None;
    }
    postprocess.get("visible")?.as_object()
}

fn postprocess_relay_is_safe(root: &Value) -> bool {
    let Some(objects) = root.get("objects").and_then(Value::as_array) else {
        return false;
    };
    let postprocess_objects = objects
        .iter()
        .filter(|object| {
            object
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "postprocess")
        })
        .collect::<Vec<_>>();
    let [postprocess] = postprocess_objects.as_slice() else {
        return false;
    };
    let Some(visible) = postprocess.get("visible").and_then(Value::as_object) else {
        return false;
    };
    let Some(script) = visible.get("script") else {
        return true;
    };
    let Some(script) = script.as_str() else {
        return false;
    };
    let Some(analysis) = analyze_script(script) else {
        return false;
    };
    if analysis_has_executable_identifier(script, &analysis, "eval")
        || analysis.has_shared_do_jump_owner
        || analysis_has_unsafe_computed_member_access(script, &analysis)
        || !non_handler_local_storage_is_safe(script, &analysis)
    {
        return false;
    }
    match analysis.cursor_down_functions.as_slice() {
        [] => {
            !analysis_has_executable_identifier(script, &analysis, "shared")
                && !analysis_has_executable_identifier(script, &analysis, "doJump")
        }
        [function] => function_is_canonical_shared_do_jump_relay(script, &analysis, function),
        _ => false,
    }
}

fn rewrite_scripts_in_value(value: &mut Value, rewritten_handlers: &mut usize) {
    match value {
        Value::Object(map) => {
            if map
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "postprocess")
            {
                return;
            }
            if let Some(Value::String(script)) = map.get("script").cloned() {
                if let Some(rewritten) = rewrite_cursor_down_script(&script) {
                    map.insert("script".into(), Value::String(rewritten));
                    *rewritten_handlers += 1;
                }
            }
            for child in map.values_mut() {
                rewrite_scripts_in_value(child, rewritten_handlers);
            }
        }
        Value::Array(items) => {
            for child in items {
                rewrite_scripts_in_value(child, rewritten_handlers);
            }
        }
        _ => {}
    }
}

/// When `cursorDown` has a non-empty body and `shared.doJump` is not already
/// assigned, extract the body into `doJump` and leave `cursorDown` empty.
fn rewrite_cursor_down_script(script: &str) -> Option<String> {
    let analysis = analyze_script(script)?;
    if analysis_has_executable_identifier(script, &analysis, "eval")
        || analysis.has_shared_do_jump_owner
    {
        return None;
    }
    let [function] = analysis.cursor_down_functions.as_slice() else {
        return None;
    };
    if analysis_has_executable_identifier(script, &analysis, "shared")
        || analysis_has_executable_identifier(script, &analysis, "doJump")
        || !cursor_handler_is_context_free(script, &analysis, function)
    {
        return None;
    }
    let body = script[function.body_start..function.body_end].trim();
    if body.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(script.len() + 64);
    out.push_str(&script[..function.function_start]);
    out.push_str("function doJump() {\n");
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("}\n\nshared.doJump = doJump;\n\n");
    out.push_str(
        "/**\n * @param {ICursorEvent} event\n */\nexport function cursorDown(event) {\n}\n",
    );
    out.push_str(&script[function.function_end..]);
    // Official Android scripts use the string locations 'global'/'default';
    // desktop engines expose constants which are undefined on mobile. Only
    // rewrite executable member-access tokens, never matching text in strings
    // or comments. Templates remain an ambiguous fail-closed case in the lexer.
    rewrite_local_storage_locations(&out)
}

#[derive(Debug)]
struct ScriptAnalysis {
    tokens: Vec<JsToken>,
    cursor_down_functions: Vec<ExportFunction>,
    has_shared_do_jump_owner: bool,
}

#[derive(Clone, Copy, Debug)]
struct JsToken {
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug)]
struct ExportFunction {
    function_start: usize,
    parameters_start: usize,
    parameters_end: usize,
    body_start: usize,
    body_end: usize,
    function_end: usize,
}

fn analyze_script(script: &str) -> Option<ScriptAnalysis> {
    let tokens = lex_javascript(script)?;
    if !delimiters_are_balanced(script, &tokens) {
        return None;
    }
    let cursor_down_functions = find_export_functions(script, &tokens, "cursorDown")?;
    let has_dot_assignment = tokens.windows(4).enumerate().any(|(index, window)| {
        token_is(script, window[0], "shared")
            && token_is(script, window[1], ".")
            && token_is(script, window[2], "doJump")
            && is_assignment_operator(script, &tokens, index + 3)
    });
    let has_computed_shared_access = tokens
        .windows(2)
        .any(|window| token_is(script, window[0], "shared") && token_is(script, window[1], "["));
    Some(ScriptAnalysis {
        tokens,
        cursor_down_functions,
        has_shared_do_jump_owner: has_dot_assignment || has_computed_shared_access,
    })
}

fn lex_javascript(script: &str) -> Option<Vec<JsToken>> {
    let bytes = script.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();

        if let Some(length) = line_terminator_sequence_len_at(bytes, i) {
            i += length;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        if c == b'/' && next == Some(b'/') {
            i += 2;
            while i < bytes.len() && line_terminator_sequence_len_at(bytes, i).is_none() {
                i += 1;
            }
            continue;
        }
        if c == b'/' && next == Some(b'*') {
            i += 2;
            let mut closed = false;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return None;
            }
            continue;
        }
        if c == b'/' {
            // Distinguishing division from a regular-expression literal needs
            // a full JavaScript grammar. Neither is required by the verified
            // Dino transform, so keep the original bytes for either form.
            return None;
        }
        if c == b'\\' {
            // JavaScript permits Unicode escapes inside identifiers. The
            // narrow scanner does not decode them, so executable backslashes
            // make ownership and shadowing proofs ambiguous.
            return None;
        }

        if c == b'`' {
            // Template interpolation contains executable JavaScript. Refuse the
            // rewrite instead of pretending the template is an inert string.
            return None;
        }
        if c == b'\'' || c == b'"' {
            let start = i;
            let quote = c;
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = i.checked_add(
                        line_terminator_sequence_len_at(bytes, i + 1)
                            .map_or(2, |length| length + 1),
                    )?;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    closed = true;
                    break;
                }
                if line_terminator_sequence_len_at(bytes, i).is_some() {
                    return None;
                }
                i += 1;
            }
            if !closed {
                return None;
            }
            tokens.push(JsToken { start, end: i });
            continue;
        }

        let start = i;
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
        } else {
            i += 1;
        }
        tokens.push(JsToken { start, end: i });
    }
    Some(tokens)
}

fn line_terminator_sequence_len_at(bytes: &[u8], index: usize) -> Option<usize> {
    match bytes.get(index).copied()? {
        b'\r' if bytes.get(index + 1) == Some(&b'\n') => Some(2),
        b'\r' | b'\n' => Some(1),
        0xE2 if bytes
            .get(index..index + 3)
            .is_some_and(|value| matches!(value, [0xE2, 0x80, 0xA8 | 0xA9])) =>
        {
            Some(3)
        }
        _ => None,
    }
}

fn delimiters_are_balanced(script: &str, tokens: &[JsToken]) -> bool {
    let mut stack = Vec::new();
    for token in tokens {
        let text = &script[token.start..token.end];
        if matches!(text, "(" | "[" | "{") {
            stack.push(text);
            continue;
        }
        let expected_open = match text {
            ")" => "(",
            "]" => "[",
            "}" => "{",
            _ => continue,
        };
        if stack.pop() != Some(expected_open) {
            return false;
        }
    }
    stack.is_empty()
}

fn find_export_functions(
    script: &str,
    tokens: &[JsToken],
    name: &str,
) -> Option<Vec<ExportFunction>> {
    let mut functions = Vec::new();
    let mut index = 0;
    while index + 2 < tokens.len() {
        if !token_is(script, tokens[index], "export")
            || !token_is(script, tokens[index + 1], "function")
            || !token_is(script, tokens[index + 2], name)
        {
            index += 1;
            continue;
        }

        let params_open = index + 3;
        if params_open >= tokens.len() || !token_is(script, tokens[params_open], "(") {
            return None;
        }
        let params_close = find_matching_token(script, tokens, params_open, "(", ")")?;
        let body_open = params_close + 1;
        if body_open >= tokens.len() || !token_is(script, tokens[body_open], "{") {
            return None;
        }
        let body_close = find_matching_token(script, tokens, body_open, "{", "}")?;
        functions.push(ExportFunction {
            function_start: tokens[index].start,
            parameters_start: tokens[params_open].end,
            parameters_end: tokens[params_close].start,
            body_start: tokens[body_open].end,
            body_end: tokens[body_close].start,
            function_end: tokens[body_close].end,
        });
        index = body_close + 1;
    }
    Some(functions)
}

fn find_matching_token(
    script: &str,
    tokens: &[JsToken],
    open_index: usize,
    open: &str,
    close: &str,
) -> Option<usize> {
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token_is(script, *token, open) {
            depth += 1;
        } else if token_is(script, *token, close) {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn token_is(script: &str, token: JsToken, expected: &str) -> bool {
    script.get(token.start..token.end) == Some(expected)
}

fn analysis_has_executable_identifier(
    script: &str,
    analysis: &ScriptAnalysis,
    identifier: &str,
) -> bool {
    analysis
        .tokens
        .iter()
        .any(|token| token_is(script, *token, identifier))
}

fn non_handler_local_storage_is_safe(script: &str, analysis: &ScriptAnalysis) -> bool {
    if !analysis_has_executable_identifier(script, analysis, "localStorage") {
        return true;
    }

    rewrite_local_storage_locations(script).is_some_and(|rewritten| rewritten == script)
}

fn analysis_has_unsafe_computed_member_access(script: &str, analysis: &ScriptAnalysis) -> bool {
    const MOBILE_STORAGE_NAME: &[u8] = b"localStorage";

    // Match square brackets in one pass. Calling `find_matching_token` for
    // every opening bracket makes a deeply nested script quadratic before any
    // expression proof runs.
    let mut brackets = Vec::new();
    for (index, token) in analysis.tokens.iter().copied().enumerate() {
        if token_is(script, token, "[") {
            brackets.push((
                index,
                computed_bracket_has_member_base(script, &analysis.tokens, index),
            ));
            continue;
        }
        if !token_is(script, token, "]") {
            continue;
        }
        let Some((open, is_member_access)) = brackets.pop() else {
            return true;
        };
        if !is_member_access {
            continue;
        }

        let expression = &analysis.tokens[open + 1..index];
        let Some(value) =
            static_computed_member_name(script, expression, MOBILE_STORAGE_NAME.len())
        else {
            // Only a string literal, parenthesized string literal, or a pure
            // `+` concatenation of those forms is proven safe. Calls,
            // identifiers, conditionals, indexes, and every other expression
            // keep the original Scene bytes.
            return true;
        };
        if value.equals(MOBILE_STORAGE_NAME) {
            return true;
        }
    }
    !brackets.is_empty()
}

const STATIC_MEMBER_EXPRESSION_MAX_DEPTH: usize = 64;

#[derive(Debug, Default)]
struct BoundedStaticString {
    bytes: Vec<u8>,
    truncated: bool,
}

impl BoundedStaticString {
    fn append(&mut self, bytes: &[u8], limit: usize) {
        let remaining = limit.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        self.truncated |= bytes.len() > remaining;
    }

    fn append_value(&mut self, value: Self, limit: usize) {
        self.append(&value.bytes, limit);
        self.truncated |= value.truncated;
    }

    fn equals(&self, expected: &[u8]) -> bool {
        !self.truncated && self.bytes == expected
    }
}

fn static_computed_member_name(
    script: &str,
    tokens: &[JsToken],
    output_limit: usize,
) -> Option<BoundedStaticString> {
    let mut parser = StaticMemberExpressionParser {
        script,
        tokens,
        index: 0,
        output_limit,
    };
    let value = parser.parse_concatenation(0)?;
    (parser.index == tokens.len()).then_some(value)
}

struct StaticMemberExpressionParser<'a> {
    script: &'a str,
    tokens: &'a [JsToken],
    index: usize,
    output_limit: usize,
}

impl StaticMemberExpressionParser<'_> {
    fn parse_concatenation(&mut self, depth: usize) -> Option<BoundedStaticString> {
        let mut value = self.parse_term(depth)?;
        while token_at_is(self.script, self.tokens, self.index, "+") {
            self.index += 1;
            let part = self.parse_term(depth)?;
            value.append_value(part, self.output_limit);
        }
        Some(value)
    }

    fn parse_term(&mut self, depth: usize) -> Option<BoundedStaticString> {
        let token = self.tokens.get(self.index).copied()?;
        if token_is_javascript_string_literal(self.script, token) {
            self.index += 1;
            return javascript_string_literal_value(self.script, token, self.output_limit);
        }
        if !token_is(self.script, token, "(") || depth >= STATIC_MEMBER_EXPRESSION_MAX_DEPTH {
            return None;
        }

        self.index += 1;
        let value = self.parse_concatenation(depth + 1)?;
        if !token_at_is(self.script, self.tokens, self.index, ")") {
            return None;
        }
        self.index += 1;
        Some(value)
    }
}

fn token_is_javascript_identifier(script: &str, token: JsToken) -> bool {
    script
        .as_bytes()
        .get(token.start)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
}

fn token_is_javascript_string_literal(script: &str, token: JsToken) -> bool {
    script
        .as_bytes()
        .get(token.start)
        .is_some_and(|byte| matches!(byte, b'\'' | b'"'))
}

fn computed_bracket_has_member_base(script: &str, tokens: &[JsToken], open_index: usize) -> bool {
    if open_index > 0 && token_can_end_member_base(script, tokens[open_index - 1]) {
        return true;
    }
    open_index >= 3
        && token_at_is(script, tokens, open_index - 1, ".")
        && token_at_is(script, tokens, open_index - 2, "?")
        && token_can_end_member_base(script, tokens[open_index - 3])
}

fn token_can_end_member_base(script: &str, token: JsToken) -> bool {
    token_is_javascript_identifier(script, token)
        || token_is(script, token, ")")
        || token_is(script, token, "]")
}

fn javascript_string_literal_value(
    script: &str,
    token: JsToken,
    output_limit: usize,
) -> Option<BoundedStaticString> {
    let raw = script.as_bytes().get(token.start..token.end)?;
    let (&quote, body_and_quote) = raw.split_first()?;
    if !matches!(quote, b'\'' | b'"') || body_and_quote.last().copied() != Some(quote) {
        return None;
    }
    let body = &body_and_quote[..body_and_quote.len() - 1];
    let mut decoded = BoundedStaticString {
        bytes: Vec::with_capacity(body.len().min(output_limit)),
        truncated: false,
    };
    let mut index = 0;
    while index < body.len() {
        if body[index] != b'\\' {
            decoded.append(&body[index..index + 1], output_limit);
            index += 1;
            continue;
        }
        let escape = body.get(index + 1).copied()?;
        if let Some(length) = line_terminator_sequence_len_at(body, index + 1) {
            index += length + 1;
            continue;
        }
        match escape {
            b'u' => {
                let (value, consumed) = if body.get(index + 2) == Some(&b'{') {
                    let digits_start = index + 3;
                    let close =
                        body[digits_start..].iter().position(|byte| *byte == b'}')? + digits_start;
                    let digits = body.get(digits_start..close)?;
                    if digits.is_empty() || digits.len() > 6 {
                        return None;
                    }
                    let digits = std::str::from_utf8(digits).ok()?;
                    (u32::from_str_radix(digits, 16).ok()?, close - index + 1)
                } else {
                    let hex = body.get(index + 2..index + 6)?;
                    let hex = std::str::from_utf8(hex).ok()?;
                    (u32::from_str_radix(hex, 16).ok()?, 6)
                };
                let character = char::from_u32(value)?;
                let mut utf8 = [0_u8; 4];
                decoded.append(character.encode_utf8(&mut utf8).as_bytes(), output_limit);
                index += consumed;
            }
            b'x' => {
                let hex = body.get(index + 2..index + 4)?;
                let Ok(hex) = std::str::from_utf8(hex) else {
                    return None;
                };
                let Ok(value) = u8::from_str_radix(hex, 16) else {
                    return None;
                };
                decoded.append(&[value], output_limit);
                index += 4;
            }
            b'n' => {
                decoded.append(b"\n", output_limit);
                index += 2;
            }
            b'r' => {
                decoded.append(b"\r", output_limit);
                index += 2;
            }
            b't' => {
                decoded.append(b"\t", output_limit);
                index += 2;
            }
            b'b' => {
                decoded.append(&[8], output_limit);
                index += 2;
            }
            b'f' => {
                decoded.append(&[12], output_limit);
                index += 2;
            }
            b'v' => {
                decoded.append(&[11], output_limit);
                index += 2;
            }
            b'0'..=b'9' => return None,
            non_ascii if !non_ascii.is_ascii() => return None,
            escaped => {
                decoded.append(&[escaped], output_limit);
                index += 2;
            }
        }
    }

    Some(decoded)
}

fn cursor_handler_is_context_free(
    script: &str,
    analysis: &ScriptAnalysis,
    function: &ExportFunction,
) -> bool {
    if analysis
        .tokens
        .iter()
        .filter(|token| token_is(script, **token, "cursorDown"))
        .count()
        != 1
    {
        // The sole executable token is the exported declaration name found by
        // `find_export_functions`. Any other reference would keep pointing at
        // the emptied handler after its body is moved into `doJump`.
        return false;
    }

    let parameter_tokens = analysis.tokens.iter().copied().filter(|token| {
        token.start >= function.parameters_start && token.end <= function.parameters_end
    });
    let parameter_tokens = parameter_tokens.collect::<Vec<_>>();
    if parameter_tokens.len() != 1 || !token_is(script, parameter_tokens[0], "event") {
        return false;
    }

    !analysis.tokens.iter().copied().any(|token| {
        token.start >= function.body_start
            && token.end <= function.body_end
            && ["event", "this", "arguments", "cursorDown"]
                .iter()
                .any(|identifier| token_is(script, token, identifier))
    })
}

fn rewrite_local_storage_locations(script: &str) -> Option<String> {
    let tokens = lex_javascript(script)?;
    if !delimiters_are_balanced(script, &tokens) {
        return None;
    }

    let mut edits = Vec::new();
    let mut target_accesses = 0_usize;
    for (index, window) in tokens.windows(3).enumerate() {
        if !token_is(script, window[0], "localStorage") || !token_is(script, window[1], ".") {
            continue;
        }
        let replacement = if token_is(script, window[2], "LOCATION_GLOBAL") {
            "'global'"
        } else if token_is(script, window[2], "LOCATION_DEFAULT") {
            "'default'"
        } else {
            continue;
        };

        // `object.localStorage.LOCATION_*` is not the verified global API.
        // Keep it unchanged instead of producing `object.'global'`.
        if index > 0 && token_is(script, tokens[index - 1], ".") {
            continue;
        }
        if !local_storage_location_access_is_read_only(script, &tokens, index) {
            return None;
        }

        target_accesses += 1;
        edits.push((window[0].start, window[0].end, replacement));
        edits.push((window[1].start, window[1].end, ""));
        edits.push((window[2].start, window[2].end, ""));
    }

    let mut verified_location_tokens = 0_usize;
    for (index, token) in tokens.iter().copied().enumerate() {
        if !token_is(script, token, "localStorage") {
            continue;
        }
        if !token_at_is(script, &tokens, index + 1, ".") {
            // Optional chaining, computed properties, bindings, and bare uses
            // are outside the verified mobile adaptation shape.
            return None;
        }
        let is_location = token_at_is(script, &tokens, index + 2, "LOCATION_GLOBAL")
            || token_at_is(script, &tokens, index + 2, "LOCATION_DEFAULT");
        if !is_location {
            // Official scripts also call global localStorage methods such as
            // get/set. Only those verified direct calls remain byte-preserved;
            // unknown constants/members and detached or chained method values
            // are not proven mobile-compatible.
            let is_verified_method = (token_at_is(script, &tokens, index + 2, "get")
                || token_at_is(script, &tokens, index + 2, "set"))
                && token_at_is(script, &tokens, index + 3, "(");
            if is_verified_method {
                continue;
            }
            return None;
        }
        if index > 0 && token_at_is(script, &tokens, index - 1, ".") {
            return None;
        }
        verified_location_tokens += 1;
    }
    if verified_location_tokens != target_accesses {
        return None;
    }

    let mut rewritten = script.to_owned();
    edits.sort_by_key(|(start, _, _)| *start);
    for (start, end, replacement) in edits.into_iter().rev() {
        rewritten.replace_range(start..end, replacement);
    }
    Some(rewritten)
}

fn local_storage_location_access_is_read_only(
    script: &str,
    tokens: &[JsToken],
    access_start: usize,
) -> bool {
    let (expression_start, expression_end) =
        expand_parenthesized_expression(script, tokens, access_start, access_start + 2);
    let after_expression = expression_end + 1;
    if is_assignment_operator(script, tokens, after_expression)
        || tokens_match(script, tokens, after_expression, &["+", "+"])
        || tokens_match(script, tokens, after_expression, &["-", "-"])
        || is_for_iteration_target(script, tokens, expression_start, after_expression)
    {
        return false;
    }

    if expression_start >= 2
        && (tokens_match(script, tokens, expression_start - 2, &["+", "+"])
            || tokens_match(script, tokens, expression_start - 2, &["-", "-"]))
    {
        return false;
    }

    if expression_start > 0 && token_at_is(script, tokens, expression_start - 1, "delete") {
        return false;
    }

    // Member expressions can be assignment targets inside array/object
    // destructuring or for-in/for-of targets. If this access is enclosed by a
    // bracket/brace whose matching close is immediately used as a target,
    // replacing it with a string literal would change valid l-value syntax
    // into an invalid assignment.
    for (open_index, token) in tokens.iter().copied().enumerate().take(access_start) {
        let Some(close) = (if token_is(script, token, "[") {
            find_matching_token(script, tokens, open_index, "[", "]")
        } else if token_is(script, token, "{") {
            find_matching_token(script, tokens, open_index, "{", "}")
        } else {
            None
        }) else {
            continue;
        };
        if close <= expression_end {
            continue;
        }
        let after_close = close + 1;
        if (is_assignment_operator(script, tokens, after_close)
            || is_for_iteration_target(script, tokens, open_index, after_close))
            && local_storage_access_is_destructuring_target(
                script,
                tokens,
                open_index,
                expression_start,
                expression_end,
            )
        {
            return false;
        }
    }

    true
}

fn local_storage_access_is_destructuring_target(
    script: &str,
    tokens: &[JsToken],
    pattern_open: usize,
    expression_start: usize,
    expression_end: usize,
) -> bool {
    // A computed object key is evaluated as a read even though its containing
    // object is an assignment pattern: `{ [LOCATION]: target } = source`.
    for (open, token) in tokens
        .iter()
        .copied()
        .enumerate()
        .take(expression_start)
        .skip(pattern_open + 1)
    {
        if !token_is(script, token, "[") {
            continue;
        }
        let Some(close) = find_matching_token(script, tokens, open, "[", "]") else {
            continue;
        };
        if close > expression_end && token_at_is(script, tokens, close + 1, ":") {
            return false;
        }
    }

    // In an assignment element such as `[target = LOCATION]`, everything
    // after the element's top-level `=` is a default-value read. Reset at each
    // top-level comma so an earlier element's default does not affect the next
    // target.
    let mut default_assignment_by_depth = vec![false];
    for (index, token) in tokens
        .iter()
        .copied()
        .enumerate()
        .take(expression_start)
        .skip(pattern_open + 1)
    {
        let text = &script[token.start..token.end];
        match text {
            "(" | "[" | "{" => default_assignment_by_depth.push(false),
            ")" | "]" | "}" => {
                default_assignment_by_depth.pop();
            }
            "," => {
                if let Some(current_element) = default_assignment_by_depth.last_mut() {
                    *current_element = false;
                }
            }
            _ if is_assignment_operator(script, tokens, index) => {
                if let Some(current_element) = default_assignment_by_depth.last_mut() {
                    *current_element = true;
                }
            }
            _ => {}
        }
    }
    !default_assignment_by_depth.iter().any(|assigned| *assigned)
}

fn expand_parenthesized_expression(
    script: &str,
    tokens: &[JsToken],
    mut expression_start: usize,
    mut expression_end: usize,
) -> (usize, usize) {
    while expression_start > 0 && token_at_is(script, tokens, expression_start - 1, "(") {
        let open = expression_start - 1;
        let Some(close) = find_matching_token(script, tokens, open, "(", ")") else {
            break;
        };
        if close != expression_end + 1 {
            break;
        }
        expression_start = open;
        expression_end = close;
    }
    (expression_start, expression_end)
}

fn token_at_is(script: &str, tokens: &[JsToken], index: usize, expected: &str) -> bool {
    tokens
        .get(index)
        .copied()
        .is_some_and(|token| token_is(script, token, expected))
}

fn tokens_match(script: &str, tokens: &[JsToken], start: usize, expected: &[&str]) -> bool {
    expected
        .iter()
        .enumerate()
        .all(|(offset, text)| token_at_is(script, tokens, start + offset, text))
}

fn is_for_iteration_target(
    script: &str,
    tokens: &[JsToken],
    target_start: usize,
    operator_index: usize,
) -> bool {
    if !(token_at_is(script, tokens, operator_index, "in")
        || token_at_is(script, tokens, operator_index, "of"))
        || target_start < 2
    {
        return false;
    }
    let ordinary_for = token_at_is(script, tokens, target_start - 1, "(")
        && token_at_is(script, tokens, target_start - 2, "for");
    let async_for = target_start >= 3
        && token_at_is(script, tokens, target_start - 1, "(")
        && token_at_is(script, tokens, target_start - 2, "await")
        && token_at_is(script, tokens, target_start - 3, "for");
    ordinary_for || async_for
}

fn is_assignment_operator(script: &str, tokens: &[JsToken], index: usize) -> bool {
    let is_plain_assignment = token_at_is(script, tokens, index, "=")
        && !(index > 0
            && ["=", "!", "<", ">"]
                .iter()
                .any(|token| token_at_is(script, tokens, index - 1, token)))
        && !["=", ">"]
            .iter()
            .any(|token| token_at_is(script, tokens, index + 1, token));
    is_plain_assignment
        || [
            &["+", "="][..],
            &["-", "="][..],
            &["*", "="][..],
            &["%", "="][..],
            &["&", "="][..],
            &["|", "="][..],
            &["^", "="][..],
            &["*", "*", "="][..],
            &["&", "&", "="][..],
            &["|", "|", "="][..],
            &["?", "?", "="][..],
            &["<", "<", "="][..],
            &[">", ">", "="][..],
            &[">", ">", ">", "="][..],
        ]
        .iter()
        .any(|operator| tokens_match(script, tokens, index, operator))
}

fn function_is_canonical_shared_do_jump_relay(
    script: &str,
    analysis: &ScriptAnalysis,
    function: &ExportFunction,
) -> bool {
    if analysis
        .tokens
        .iter()
        .filter(|token| token_is(script, **token, "shared"))
        .count()
        != 1
    {
        return false;
    }

    let parameter_tokens = analysis
        .tokens
        .iter()
        .copied()
        .filter(|token| {
            token.start >= function.parameters_start && token.end <= function.parameters_end
        })
        .collect::<Vec<_>>();
    if parameter_tokens.len() != 1 || !token_is(script, parameter_tokens[0], "event") {
        return false;
    }

    let body_tokens = analysis
        .tokens
        .iter()
        .copied()
        .filter(|token| token.start >= function.body_start && token.end <= function.body_end);
    let body_tokens = body_tokens.collect::<Vec<_>>();
    if body_tokens.len() != 5 && body_tokens.len() != 6 {
        return false;
    }
    token_is(script, body_tokens[0], "shared")
        && token_is(script, body_tokens[1], ".")
        && token_is(script, body_tokens[2], "doJump")
        && token_is(script, body_tokens[3], "(")
        && token_is(script, body_tokens[4], ")")
        && (body_tokens.len() == 5 || token_is(script, body_tokens[5], ";"))
}

fn ensure_postprocess_cursor_relay(root: &mut Value) -> bool {
    let Some(objects) = root.get_mut("objects").and_then(Value::as_array_mut) else {
        return false;
    };
    let postprocess_indices = objects
        .iter()
        .enumerate()
        .filter_map(|(index, object)| {
            object
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "postprocess")
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let [postprocess_index] = postprocess_indices.as_slice() else {
        return false;
    };
    let Some(visible) = objects[*postprocess_index]
        .get_mut("visible")
        .and_then(Value::as_object_mut)
    else {
        return false;
    };

    match visible.get_mut("script") {
        None => {
            visible.insert(
                "script".into(),
                Value::String(POSTPROCESS_RELAY_SCRIPT.to_owned()),
            );
            true
        }
        Some(Value::String(script)) => {
            let Some(analysis) = analyze_script(script) else {
                return false;
            };
            if let [function] = analysis.cursor_down_functions.as_slice() {
                return function_is_canonical_shared_do_jump_relay(script, &analysis, function);
            }

            if !script.is_empty() {
                if !script.ends_with('\n') {
                    script.push('\n');
                }
                script.push('\n');
            }
            script.push_str(POSTPROCESS_CURSOR_RELAY);
            true
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_ambiguous_source_script_is_byte_stable(ambiguous_script: &str) {
        let scene = json!({
            "objects": [
                {
                    "name": "ambiguous",
                    "visible": { "script": ambiguous_script }
                },
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes, "ambiguous script: {ambiguous_script}");
    }

    fn assert_postprocess_relay_is_byte_stable(relay: &str) {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true, "script": relay }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes, "ambiguous relay: {relay}");
    }

    #[test]
    fn rewrites_non_empty_cursor_down_and_adds_postprocess_relay() {
        let scene = json!({
            "objects": [
                {
                    "name": "mario_walk_1",
                    "visible": {
                        "script": "export function cursorDown(event) {\njumpVelocity = 700;\n}\nexport function update() {}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true, "user": false }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();
        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let mario = v["objects"][0]["visible"]["script"].as_str().unwrap();
        assert!(mario.contains("function doJump()"));
        assert!(mario.contains("shared.doJump = doJump"));
        assert!(mario.contains("jumpVelocity = 700;"));
        // cursorDown body must be empty after rewrite (jump lives in doJump).
        assert!(mario.contains("export function cursorDown(event) {\n}"));
        let post = v["objects"][1]["visible"]["script"].as_str().unwrap();
        assert!(post.contains("shared.doJump()"));
        assert_eq!(v["objects"][1]["visible"]["value"], true);
        assert_eq!(v["objects"][1]["visible"]["user"], false);
    }

    #[test]
    fn skips_when_shared_do_jump_already_present() {
        let script = "function doJump(){ x(); }\nshared.doJump = doJump;\nexport function cursorDown(event) {\n}\n";
        let scene = json!({
            "objects": [{
                "name": "mario_walk_1",
                "visible": { "script": script }
            }]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();
        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v["objects"][0]["visible"]["script"].as_str().unwrap(),
            script
        );
    }

    #[test]
    fn preserves_existing_postprocess_script_when_adding_relay() {
        let postprocess_script = "export function update(value) {\nreturn !value;\n}\n";
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": {
                        "value": true,
                        "script": postprocess_script
                    }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let post = v["objects"][1]["visible"]["script"].as_str().unwrap();

        assert!(post.starts_with(postprocess_script));
        assert!(post.contains("return !value;"));
        assert!(post.contains("shared.doJump();"));
    }

    #[test]
    fn leaves_scene_unchanged_without_postprocess_relay_target() {
        let scene = json!({
            "objects": [{
                "name": "interactive",
                "visible": {
                    "script": "export function cursorDown(event) {\njump();\n}\n"
                }
            }]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn leaves_scene_unchanged_when_multiple_handlers_would_compete_for_shared_jump() {
        let scene = json!({
            "objects": [
                {
                    "name": "first",
                    "visible": {
                        "script": "export function cursorDown(event) {\nfirst();\n}\n"
                    }
                },
                {
                    "name": "second",
                    "visible": {
                        "script": "export function cursorDown(event) {\nsecond();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn leaves_scene_unchanged_when_postprocess_has_an_unrelated_cursor_handler() {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": {
                        "value": true,
                        "script": "export function cursorDown(event) {\ncustomPostprocessAction();\n}\n"
                    }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn reuses_existing_postprocess_shared_jump_relay() {
        let relay = "export function cursorDown(event) {\nshared.doJump();\n}\n";
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": {
                        "value": true,
                        "script": relay
                    }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            v["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
        assert_eq!(v["objects"][1]["visible"]["script"], relay);
    }

    #[test]
    fn leaves_scene_unchanged_when_postprocess_visible_is_not_an_object() {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": true
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn leaves_scene_unchanged_when_another_script_owns_shared_do_jump() {
        let scene = json!({
            "objects": [
                {
                    "name": "owner",
                    "visible": {
                        "script": "function existingJump() {}\nshared.doJump = existingJump;\n"
                    }
                },
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn rejects_postprocess_text_that_is_not_a_shared_do_jump_call() {
        let invalid_bodies = [
            "shared.doJumpElse();",
            "// shared.doJump();\ncustom();",
            "const message = 'shared.doJump()';\ncustom();",
            "const jump = shared.doJump;\ncustom();",
            "shared.doJump = custom;",
        ];

        for invalid_body in invalid_bodies {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": "export function cursorDown(event) {\njump();\n}\n"
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": {
                            "value": true,
                            "script": format!(
                                "export function cursorDown(event) {{\n{invalid_body}\n}}\n"
                            )
                        }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "invalid relay body: {invalid_body}");
        }
    }

    #[test]
    fn rejects_qualified_shared_do_jump_calls() {
        for relay in [
            "export function cursorDown(event) {\nevent.shared.doJump();\n}\n",
            "export function cursorDown(event) {\nthis.shared.doJump();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn rejects_shared_parameter_shadowing() {
        assert_postprocess_relay_is_byte_stable(
            "export function cursorDown(shared) {\nshared.doJump();\n}\n",
        );
    }

    #[test]
    fn rejects_shared_local_shadowing() {
        for relay in [
            "export function cursorDown(event) {\nlet shared = event;\nshared.doJump();\n}\n",
            "export function cursorDown(event) {\nfunction shared() {}\nshared.doJump();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn rejects_module_scope_variable_shared_bindings() {
        for relay in [
            "const shared = fake;\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "let shared = fake;\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "var shared = fake;\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn rejects_module_scope_function_class_and_import_shared_bindings() {
        for relay in [
            "function shared() {}\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "class shared {}\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "import shared from 'module';\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn rejects_unicode_escaped_variable_shared_bindings() {
        for relay in [
            "const sh\\u0061red = fake;\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "let \\u0073hared = fake;\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn rejects_unicode_escaped_function_class_and_import_bindings() {
        for relay in [
            "function sh\\u0061red() {}\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "class \\u0073hared {}\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
            "import sh\\u0061red from 'module';\nexport function cursorDown(event) {\nshared.doJump();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn rejects_noncanonical_shared_do_jump_expressions() {
        for relay in [
            "export function cursorDown(event) {\nshared?.doJump();\n}\n",
            "export function cursorDown(event) {\nshared.doJump(event);\n}\n",
            "export function cursorDown(event) {\nshared.doJump().then(done);\n}\n",
            "export function cursorDown(event) {\nshared.doJump();\ncustom();\n}\n",
        ] {
            assert_postprocess_relay_is_byte_stable(relay);
        }
    }

    #[test]
    fn reuses_canonical_relay_with_inert_comments_and_whitespace() {
        let relay = concat!(
            "export function cursorDown( event ) {\n",
            "// relay to the source handler\n",
            "shared /* global */ . doJump ( ) ;\n",
            "}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true, "script": relay }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            transformed["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
        assert_eq!(transformed["objects"][1]["visible"]["script"], relay);
    }

    #[test]
    fn reuses_canonical_relay_when_other_shared_text_is_inert() {
        let relay = concat!(
            "// const shared = commentOnly;\n",
            "const example = 'shared';\n",
            "export function cursorDown(event) {\n",
            "shared.doJump();\n",
            "}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true, "script": relay }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            transformed["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
        assert_eq!(transformed["objects"][1]["visible"]["script"], relay);
    }

    #[test]
    fn reuses_canonical_relay_when_unicode_escape_text_is_inert() {
        let relay = concat!(
            "// const sh\\u0061red = commentOnly;\n",
            "const example = '\\u0073hared';\n",
            "export function cursorDown(event) {\n",
            "shared.doJump();\n",
            "}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true, "script": relay }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            transformed["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
        assert_eq!(transformed["objects"][1]["visible"]["script"], relay);
    }

    #[test]
    fn reuses_postprocess_relay_with_extra_export_spacing() {
        let relay = "export  function cursorDown(event) {\nshared.doJump();\n}\n";
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": {
                        "value": true,
                        "script": relay
                    }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            value["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
        assert_eq!(value["objects"][1]["visible"]["script"], relay);
    }

    #[test]
    fn ignores_cursor_down_declarations_inside_comments_and_strings() {
        for fake in [
            "// export function cursorDown(event) { fake(); }\n",
            "const example = 'export function cursorDown(event) { fake(); }';\n",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "fake",
                        "visible": { "script": fake }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "fake declaration: {fake}");
        }
    }

    #[test]
    fn leaves_scene_unchanged_when_a_template_makes_preflight_ambiguous() {
        let scene = json!({
            "objects": [
                {
                    "name": "template-user",
                    "visible": { "script": "const message = `${shared.doJump = hidden}`;\n" }
                },
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn leaves_scene_unchanged_for_every_executable_slash_context() {
        for ambiguous_script in [
            "check() / harmless /;\n",
            "function check() {} / harmless /;\n",
            "count++ / harmless /;\n",
            "; /harmless/;\n",
            "const ratio = width / height;\n",
        ] {
            assert_ambiguous_source_script_is_byte_stable(ambiguous_script);
        }
    }

    #[test]
    fn leaves_scene_unchanged_when_source_delimiters_are_malformed() {
        for malformed_script in [
            "function malformed( { }\n",
            "function malformed() { return ([)]; }\n",
            "function malformed() {\n",
        ] {
            assert_ambiguous_source_script_is_byte_stable(malformed_script);
        }
    }

    #[test]
    fn leaves_scene_unchanged_when_postprocess_relay_call_is_malformed() {
        let malformed_relay = "export function cursorDown(event) {\nshared.doJump(\n}\n)\n";
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true, "script": malformed_relay }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn leaves_scene_unchanged_for_computed_shared_property_access() {
        for computed_owner in [
            "shared['doJump'] = existingJump;\n",
            "shared[\"doJump\"] = existingJump;\n",
            "shared[dynamicProperty] = existingJump;\n",
        ] {
            assert_ambiguous_source_script_is_byte_stable(computed_owner);
        }
    }

    #[test]
    fn ignores_computed_shared_property_text_in_comments_and_strings() {
        let harmless_text = concat!(
            "// shared['doJump'] = commentOnly;\n",
            "const example = \"shared[dynamicProperty] = stringOnly\";\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "documentation",
                    "visible": { "script": harmless_text }
                },
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            transformed["objects"][0]["visible"]["script"],
            harmless_text
        );
        assert!(
            transformed["objects"][1]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
    }

    #[test]
    fn leaves_scene_unchanged_for_multiple_cursor_down_declarations_in_one_script() {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": concat!(
                            "export function cursorDown(event) {\nfirst();\n}\n",
                            "export function cursorDown(event) {\nsecond();\n}\n"
                        )
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn leaves_context_dependent_cursor_handlers_byte_stable() {
        for body in [
            "jumpAt(event.x);",
            "this.jump();",
            "jumpWith(arguments[0]);",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "export function cursorDown(event) {{\n{body}\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "context-dependent body: {body}");
        }
    }

    #[test]
    fn leaves_source_identifier_conflicts_byte_stable() {
        for declaration in [
            "const shared = fake;",
            "function shared() {}",
            "const doJump = fake;",
            "function doJump() {}",
            "const sh\\u0061red = fake;",
            "const doJ\\u0075mp = fake;",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "{declaration}\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "source declaration: {declaration}");
        }
    }

    #[test]
    fn leaves_postprocess_append_target_shadowing_byte_stable() {
        for declaration in [
            "const shared = fake;",
            "function shared() {}",
            "const sh\\u0061red = fake;",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": "export function cursorDown(event) {\njump();\n}\n"
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": {
                            "value": true,
                            "script": format!(
                                "{declaration}\nexport function update(value) {{ return value; }}\n"
                            )
                        }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "postprocess declaration: {declaration}");
        }
    }

    #[test]
    fn rewrites_only_executable_local_storage_location_tokens() {
        let source_script = concat!(
            "const globalLocation = localStorage /* keep-global-comment */ . LOCATION_GLOBAL;\n",
            "const defaultLocation = localStorage.LOCATION_DEFAULT;\n",
            "const inert = 'localStorage.LOCATION_GLOBAL';\n",
            "// localStorage.LOCATION_DEFAULT must remain documentation\n",
            "export function cursorDown(event) {\njump();\n}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": { "script": source_script }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();
        let script = transformed["objects"][0]["visible"]["script"]
            .as_str()
            .unwrap();

        assert!(script.contains("const globalLocation = 'global' /* keep-global-comment */"));
        assert!(script.contains("const defaultLocation = 'default';"));
        assert!(script.contains("const inert = 'localStorage.LOCATION_GLOBAL';"));
        assert!(script.contains("// localStorage.LOCATION_DEFAULT must remain documentation"));
    }

    #[test]
    fn leaves_local_storage_location_write_contexts_byte_stable() {
        for write_statement in [
            "localStorage.LOCATION_GLOBAL = value;",
            "localStorage.LOCATION_GLOBAL += value;",
            "localStorage.LOCATION_GLOBAL -= value;",
            "localStorage.LOCATION_GLOBAL *= value;",
            "localStorage.LOCATION_GLOBAL /= value;",
            "localStorage.LOCATION_GLOBAL %= value;",
            "localStorage.LOCATION_GLOBAL **= value;",
            "localStorage.LOCATION_GLOBAL <<= value;",
            "localStorage.LOCATION_GLOBAL >>= value;",
            "localStorage.LOCATION_GLOBAL >>>= value;",
            "localStorage.LOCATION_GLOBAL &= value;",
            "localStorage.LOCATION_GLOBAL |= value;",
            "localStorage.LOCATION_GLOBAL ^= value;",
            "localStorage.LOCATION_GLOBAL &&= value;",
            "localStorage.LOCATION_GLOBAL ||= value;",
            "localStorage.LOCATION_GLOBAL ??= value;",
            "localStorage.LOCATION_GLOBAL++;",
            "localStorage.LOCATION_GLOBAL--;",
            "++localStorage.LOCATION_GLOBAL;",
            "--localStorage.LOCATION_GLOBAL;",
            "(localStorage.LOCATION_GLOBAL) = value;",
            "(localStorage.LOCATION_GLOBAL) += value;",
            "(localStorage.LOCATION_GLOBAL)++;",
            "--(localStorage.LOCATION_GLOBAL);",
            "delete localStorage.LOCATION_GLOBAL;",
            "delete (localStorage.LOCATION_GLOBAL);",
            "[localStorage.LOCATION_GLOBAL] = values;",
            "({ value: localStorage.LOCATION_GLOBAL } = source);",
            "for (localStorage.LOCATION_GLOBAL of values) {}",
            "for (localStorage.LOCATION_GLOBAL in values) {}",
            "for ((localStorage.LOCATION_GLOBAL) of values) {}",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "{write_statement}\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "write context: {write_statement}");
        }
    }

    #[test]
    fn keeps_inert_local_storage_write_text_while_rewriting_reads() {
        let source_script = concat!(
            "// localStorage.LOCATION_GLOBAL = documentationOnly;\n",
            "const example = 'localStorage.LOCATION_DEFAULT++;';\n",
            "const location = localStorage.LOCATION_GLOBAL;\n",
            "export function cursorDown(event) {\njump();\n}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": { "script": source_script }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();
        let script = transformed["objects"][0]["visible"]["script"]
            .as_str()
            .unwrap();

        assert!(script.contains("// localStorage.LOCATION_GLOBAL = documentationOnly;"));
        assert!(script.contains("'localStorage.LOCATION_DEFAULT++;'"));
        assert!(script.contains("const location = 'global';"));
    }

    #[test]
    fn rewrites_local_storage_reads_inside_destructuring_syntax() {
        let source_script = concat!(
            "let selected;\n",
            "[selected = localStorage.LOCATION_GLOBAL] = values;\n",
            "({ [localStorage.LOCATION_DEFAULT]: selected } = source);\n",
            "export function cursorDown(event) {\njump();\n}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": { "script": source_script }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();
        let script = transformed["objects"][0]["visible"]["script"]
            .as_str()
            .unwrap();

        assert!(script.contains("[selected = 'global'] = values;"));
        assert!(script.contains("({ ['default']: selected } = source);"));
    }

    #[test]
    fn rewrites_nested_destructuring_default_reads() {
        let source_script = concat!(
            "let selected;\n",
            "({ key: [selected = localStorage.LOCATION_GLOBAL] } = source);\n",
            "export function cursorDown(event) {\njump();\n}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": { "script": source_script }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();
        let script = transformed["objects"][0]["visible"]["script"]
            .as_str()
            .unwrap();

        assert!(script.contains("{ key: [selected = 'global'] }"));
    }

    #[test]
    fn rewrites_comparison_arrow_and_membership_reads() {
        let source_script = concat!(
            "const equal = localStorage.LOCATION_GLOBAL == expected;\n",
            "const strictEqual = localStorage.LOCATION_DEFAULT === expected;\n",
            "const notEqual = localStorage.LOCATION_GLOBAL != expected;\n",
            "const strictNotEqual = localStorage.LOCATION_DEFAULT !== expected;\n",
            "const atMost = localStorage.LOCATION_GLOBAL <= expected;\n",
            "const getter = value => localStorage.LOCATION_DEFAULT;\n",
            "const present = localStorage.LOCATION_GLOBAL in locations;\n",
            "let selected;\n",
            "[selected = value => localStorage.LOCATION_DEFAULT] = values;\n",
            "export function cursorDown(event) {\njump();\n}\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": { "script": source_script }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();
        let script = transformed["objects"][0]["visible"]["script"]
            .as_str()
            .unwrap();

        assert!(!script.contains("localStorage.LOCATION_"));
        assert!(script.contains("const equal = 'global' == expected;"));
        assert!(script.contains("const strictEqual = 'default' === expected;"));
        assert!(script.contains("const present = 'global' in locations;"));
        assert!(script.contains("[selected = value => 'default'] = values;"));
    }

    #[test]
    fn leaves_cursor_down_body_self_references_byte_stable() {
        for body in [
            "cursorDown();",
            "const retry = cursorDown;\nretry();",
            "const handlers = { cursorDown };",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "export function cursorDown(event) {{\n{body}\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "self-reference body: {body}");
        }
    }

    #[test]
    fn leaves_cursor_down_references_outside_the_declaration_byte_stable() {
        for other_reference in [
            "export function helper() { cursorDown(); }",
            "const handler = cursorDown;",
            "export function helper() { return { cursorDown }; }",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "{other_reference}\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "outside reference: {other_reference}");
        }
    }

    #[test]
    fn cursor_down_text_in_body_comments_and_strings_remains_inert() {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": concat!(
                            "export function cursorDown(event) {\n",
                            "// cursorDown() is documentation only.\n",
                            "const name = 'cursorDown';\n",
                            "jump();\n",
                            "}\n"
                        )
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            transformed["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("function doJump()")
        );
    }

    #[test]
    fn leaves_eval_driven_cursor_handlers_byte_stable() {
        for hidden_context in [
            "event.x",
            "this.jump()",
            "arguments[0]",
            "shared.doJump = hiddenJump",
            "const doJump = hiddenJump",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "export function cursorDown(event) {{\neval({hidden_context:?});\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "eval payload: {hidden_context}");
        }
    }

    #[test]
    fn leaves_scene_unchanged_when_another_source_script_executes_eval() {
        assert_ambiguous_source_script_is_byte_stable(
            "eval('shared.doJump = hiddenJump; const doJump = hiddenJump');\n",
        );
    }

    #[test]
    fn leaves_postprocess_eval_target_byte_stable() {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": {
                        "value": true,
                        "script": concat!(
                            "eval('const shared = fake; shared.doJump = hiddenJump');\n",
                            "export function update(value) { return value; }\n"
                        )
                    }
                }
            ]
        });
        let bytes = serde_json::to_vec(&scene).unwrap();

        let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

        assert_eq!(out, bytes);
    }

    #[test]
    fn eval_text_in_comments_and_strings_remains_inert() {
        let inert_eval = concat!(
            "// eval('shared.doJump = commentOnly');\n",
            "const example = \"eval('event.x; this.jump(); arguments[0]')\";\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "documentation",
                    "visible": { "script": inert_eval }
                },
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(transformed["objects"][0]["visible"]["script"], inert_eval);
        assert!(
            transformed["objects"][1]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
    }

    #[test]
    fn leaves_shadowed_local_storage_location_access_byte_stable() {
        for shadowing_script in [
            "const localStorage = fake;\nconst location = localStorage.LOCATION_GLOBAL;",
            "let localStorage = fake;\nconst location = localStorage.LOCATION_GLOBAL;",
            "var localStorage = fake;\nconst location = localStorage.LOCATION_GLOBAL;",
            "function localStorage() {}\nconst location = localStorage.LOCATION_GLOBAL;",
            "class localStorage {}\nconst location = localStorage.LOCATION_GLOBAL;",
            "import localStorage from 'module';\nconst location = localStorage.LOCATION_GLOBAL;",
            "const { localStorage } = fake;\nconst location = localStorage.LOCATION_GLOBAL;",
            "function helper(localStorage) { return localStorage.LOCATION_GLOBAL; }",
            "const local\\u0053torage = fake;\nconst location = localStorage.LOCATION_GLOBAL;",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "{shadowing_script}\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "shadowing script: {shadowing_script}");
        }
    }

    #[test]
    fn audits_ambiguous_scripts_nested_below_postprocess() {
        for nested_script in [
            "eval('shared.doJump = hiddenJump');\n",
            "function hiddenJump() {}\nshared.doJump = hiddenJump;\n",
            "const sh\\u0061red = fake;\n",
            "const doJ\\u0075mp = fake;\n",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": "export function cursorDown(event) {\njump();\n}\n"
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true },
                        "effects": [
                            {
                                "passes": [
                                    { "script": nested_script }
                                ]
                            }
                        ]
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "nested script: {nested_script}");
        }
    }

    #[test]
    fn allows_benign_scripts_nested_below_postprocess() {
        let nested_script = concat!(
            "// Official Dino-style nested effect helper.\n",
            "export function update(value) { return value; }\n"
        );
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": "export function cursorDown(event) {\njump();\n}\n"
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true },
                    "effects": [
                        {
                            "passes": [
                                { "script": nested_script }
                            ]
                        }
                    ]
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();

        assert!(
            transformed["objects"][0]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump = doJump")
        );
        assert_eq!(
            transformed["objects"][1]["effects"][0]["passes"][0]["script"],
            nested_script
        );
        assert!(
            transformed["objects"][1]["visible"]["script"]
                .as_str()
                .unwrap()
                .contains("shared.doJump()")
        );
    }

    #[test]
    fn leaves_for_await_local_storage_location_targets_byte_stable() {
        for target in [
            "localStorage.LOCATION_GLOBAL",
            "(localStorage.LOCATION_GLOBAL)",
            "[localStorage.LOCATION_GLOBAL]",
            "{ key: localStorage.LOCATION_GLOBAL }",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "async function consume(values) {{\nfor /* gap */ await\n({target} of values) {{}}\n}}\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "for-await target: {target}");
        }
    }

    #[test]
    fn leaves_unsupported_local_storage_location_accesses_byte_stable() {
        for access in [
            "localStorage?.LOCATION_GLOBAL",
            "localStorage['LOCATION_GLOBAL']",
            "localStorage[\"LOCATION_DEFAULT\"]",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "const location = {access};\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "unsupported access: {access}");
        }
    }

    #[test]
    fn rewrites_locations_alongside_official_local_storage_methods() {
        let scene = json!({
            "objects": [
                {
                    "name": "interactive",
                    "visible": {
                        "script": concat!(
                            "localStorage.set('score', 1, localStorage.LOCATION_GLOBAL);\n",
                            "const score = localStorage.get('score', localStorage.LOCATION_DEFAULT);\n",
                            "export function cursorDown(event) {\njump();\n}\n"
                        )
                    }
                },
                {
                    "name": "postprocess",
                    "visible": { "value": true }
                }
            ]
        });

        let out =
            transform_scene_json_for_mobile_input(&serde_json::to_vec(&scene).unwrap()).unwrap();
        let transformed: Value = serde_json::from_slice(&out).unwrap();
        let script = transformed["objects"][0]["visible"]["script"]
            .as_str()
            .unwrap();

        assert!(script.contains("localStorage.set('score', 1, 'global')"));
        assert!(script.contains("localStorage.get('score', 'default')"));
        assert!(script.contains("shared.doJump = doJump"));
    }

    #[test]
    fn leaves_unverified_local_storage_members_byte_stable() {
        for access in [
            "localStorage.LOCATION_CUSTOM",
            "localStorage.clear()",
            "const get = localStorage.get",
            "localStorage.set.call(null, 'score', 1)",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "interactive",
                        "visible": {
                            "script": format!(
                                "const global = localStorage.LOCATION_GLOBAL;\n{access};\nexport function cursorDown(event) {{\njump();\n}}\n"
                            )
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "unverified member: {access}");
        }
    }

    #[test]
    fn leaves_sibling_and_postprocess_local_storage_scripts_byte_stable() {
        for (sibling_script, postprocess_script) in [
            ("const location = localStorage.LOCATION_GLOBAL;", None),
            ("localStorage.clear();", None),
            (
                "const location = globalThis['localStorage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = window[\"localStorage\"].LOCATION_DEFAULT;",
                None,
            ),
            (
                "const location = globalThis['local\\u0053torage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis['local\\u{53}torage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis?.['localStorage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis['local\\\nStorage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis['local\\\u{2028}Storage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis['local\\\u{2029}Storage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis['local' + ('Storage')].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis[[('local' + 'Storage')][0]].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis[true ? ('local' + 'Storage') : 'safe'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis['localStorag' + String.fromCharCode(101)].LOCATION_GLOBAL;",
                None,
            ),
            (
                "const location = globalThis[flag ? 'local' : 'Storage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "// comment\u{2028}const location = globalThis['localStorage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "// comment\u{2029}const location = globalThis['localStorage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "// comment\rconst location = globalThis['localStorage'].LOCATION_GLOBAL;",
                None,
            ),
            (
                "// comment\r\nconst location = globalThis['localStorage'].LOCATION_GLOBAL;",
                None,
            ),
            ("const text = 'before\u{2028}after';", None),
            ("const text = 'before\u{2029}after';", None),
            (
                "export function update(value) { return value; }",
                Some(
                    "const location = localStorage.LOCATION_DEFAULT;\nexport function update(value) { return value; }\n",
                ),
            ),
            (
                "export function update(value) { return value; }",
                Some(
                    "const location = globalThis['localStorage'].LOCATION_DEFAULT;\nexport function update(value) { return value; }\n",
                ),
            ),
        ] {
            let postprocess_visible = postprocess_script.map_or_else(
                || json!({ "value": true }),
                |script| json!({ "value": true, "script": script }),
            );
            let scene = json!({
                "objects": [
                    {
                        "name": "sibling",
                        "visible": { "script": sibling_script }
                    },
                    {
                        "name": "interactive",
                        "visible": {
                            "script": "export function cursorDown(event) {\njump();\n}\n"
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": postprocess_visible
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_eq!(out, bytes, "sibling: {sibling_script}");
        }
    }

    #[test]
    fn allows_verified_sibling_and_postprocess_local_storage_methods() {
        for (sibling_script, postprocess_script) in [
            (
                "export function update(value) { return localStorage.get('score', 'global'); }",
                None,
            ),
            (
                "export function update(value) { return value; }",
                Some(
                    "export function update(value) { localStorage.set('score', value, 'global'); }\n",
                ),
            ),
        ] {
            let postprocess_visible = postprocess_script.map_or_else(
                || json!({ "value": true }),
                |script| json!({ "value": true, "script": script }),
            );
            let scene = json!({
                "objects": [
                    {
                        "name": "sibling",
                        "visible": { "script": sibling_script }
                    },
                    {
                        "name": "interactive",
                        "visible": {
                            "script": "export function cursorDown(event) {\njump();\n}\n"
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": postprocess_visible
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_ne!(out, bytes, "sibling: {sibling_script}");
            let rewritten: Value = serde_json::from_slice(&out).unwrap();
            let objects = rewritten["objects"].as_array().unwrap();
            assert!(
                objects[1]["visible"]["script"]
                    .as_str()
                    .unwrap()
                    .contains("shared.doJump = doJump")
            );
            assert!(
                objects[2]["visible"]["script"]
                    .as_str()
                    .unwrap()
                    .contains("shared.doJump()")
            );
        }
    }

    #[test]
    fn allows_inert_array_and_unrelated_computed_property_text() {
        for sibling_script in [
            "const names = ['localStorage'];",
            "const value = object['unrelated'];",
            "const value = object['not\\\nLocalStorage'];",
            "const value = object['not\\\rLocalStorage'];",
            "const value = object['not\\\r\nLocalStorage'];",
            "const value = object['not\\\u{2028}LocalStorage'];",
            "const value = object['not\\\u{2029}LocalStorage'];",
        ] {
            let scene = json!({
                "objects": [
                    {
                        "name": "sibling",
                        "visible": { "script": sibling_script }
                    },
                    {
                        "name": "interactive",
                        "visible": {
                            "script": "export function cursorDown(event) {\njump();\n}\n"
                        }
                    },
                    {
                        "name": "postprocess",
                        "visible": { "value": true }
                    }
                ]
            });
            let bytes = serde_json::to_vec(&scene).unwrap();

            let out = transform_scene_json_for_mobile_input(&bytes).unwrap();

            assert_ne!(out, bytes, "sibling: {sibling_script}");
        }
    }
}
