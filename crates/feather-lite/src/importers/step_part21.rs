//! Shared ISO 10303-21 record and scalar parsing helpers.
//!
//! STEP visual importers use this module so tessellated and B-Rep paths agree
//! on record framing, references, strings, comments, and numeric syntax.

use crate::importer::ImportError;

/// One component within a simple or complex STEP entity instance.
#[derive(Debug)]
pub struct StepComponent {
    pub kind: String,
    pub args: String,
}

/// One entity instance from a STEP Part 21 DATA section.
///
/// `kind` and `args` expose the first component for existing simple-entity
/// consumers. `components` retains every component of a complex instance such
/// as the units emitted by common AP203/AP214 exporters.
#[derive(Debug)]
pub struct StepRecord {
    pub id: usize,
    pub kind: String,
    pub args: String,
    pub components: Vec<StepComponent>,
}

impl StepRecord {
    /// Returns the named component from a simple or complex entity instance.
    pub fn component(&self, kind: &str) -> Option<&StepComponent> {
        self.components
            .iter()
            .find(|component| component.kind == kind)
    }
}

/// Parses simple and complex `#id=...` records from a STEP Part 21 document.
pub fn parse_step_records(text: &str) -> Result<Vec<StepRecord>, ImportError> {
    let text = strip_step_comments(text);
    let chars = text.chars().collect::<Vec<_>>();
    let mut records = Vec::new();
    let mut cursor = 0;

    while cursor < chars.len() {
        if chars[cursor] != '#' {
            cursor += 1;
            continue;
        }

        cursor += 1;
        let id_start = cursor;
        while cursor < chars.len() && chars[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if id_start == cursor {
            continue;
        }
        let id = chars[id_start..cursor]
            .iter()
            .collect::<String>()
            .parse::<usize>()
            .map_err(|error| ImportError::InvalidData(format!("invalid STEP id: {error}")))?;

        skip_whitespace(&chars, &mut cursor);
        if chars.get(cursor) != Some(&'=') {
            continue;
        }
        cursor += 1;
        skip_whitespace(&chars, &mut cursor);

        let components = if chars.get(cursor) == Some(&'(') {
            let (body, next_cursor) = read_parenthesized(&chars, cursor)?;
            cursor = next_cursor;
            parse_complex_components(&body, id)?
        } else {
            let (component, next_cursor) = parse_component(&chars, cursor, id)?;
            cursor = next_cursor;
            vec![component]
        };
        let Some(first) = components.first() else {
            return Err(ImportError::InvalidData(format!(
                "STEP complex entity #{id} contains no components"
            )));
        };
        records.push(StepRecord {
            id,
            kind: first.kind.clone(),
            args: first.args.clone(),
            components,
        });
    }

    Ok(records)
}

fn parse_complex_components(body: &str, id: usize) -> Result<Vec<StepComponent>, ImportError> {
    let chars = body.chars().collect::<Vec<_>>();
    let mut components = Vec::new();
    let mut cursor = 0;
    while cursor < chars.len() {
        skip_whitespace(&chars, &mut cursor);
        if cursor == chars.len() {
            break;
        }
        let (component, next_cursor) = parse_component(&chars, cursor, id)?;
        components.push(component);
        cursor = next_cursor;
    }
    Ok(components)
}

fn parse_component(
    chars: &[char],
    mut cursor: usize,
    id: usize,
) -> Result<(StepComponent, usize), ImportError> {
    let kind_start = cursor;
    while cursor < chars.len() && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_') {
        cursor += 1;
    }
    if kind_start == cursor {
        return Err(ImportError::InvalidData(format!(
            "STEP entity #{id} has an invalid component name"
        )));
    }
    let kind = chars[kind_start..cursor]
        .iter()
        .collect::<String>()
        .to_ascii_uppercase();
    skip_whitespace(chars, &mut cursor);
    if chars.get(cursor) != Some(&'(') {
        return Err(ImportError::InvalidData(format!(
            "STEP entity #{id} component {kind} has no argument list"
        )));
    }
    let (args, next_cursor) = read_parenthesized(chars, cursor)?;
    Ok((StepComponent { kind, args }, next_cursor))
}

/// Splits a STEP argument list without splitting nested lists or strings.
pub fn split_top_level_args(value: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut in_string = false;
    let chars = value.char_indices().collect::<Vec<_>>();
    let mut index = 0;

    while index < chars.len() {
        let (byte_index, character) = chars[index];

        if character == '\'' {
            if in_string && chars.get(index + 1).map(|(_, c)| *c) == Some('\'') {
                index += 2;
                continue;
            }
            in_string = !in_string;
        } else if !in_string {
            match character {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    args.push(value[start..byte_index].trim());
                    start = byte_index + 1;
                }
                _ => {}
            }
        }

        index += 1;
    }

    args.push(value[start..].trim());
    args
}

/// Parses one direct `#id` reference.
pub fn parse_reference(value: &str) -> Option<usize> {
    value
        .trim()
        .strip_prefix('#')
        .and_then(|value| value.parse::<usize>().ok())
}

/// Parses one quoted STEP string and unescapes doubled apostrophes.
pub fn parse_step_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('\'') || !value.ends_with('\'') {
        return None;
    }
    Some(value[1..value.len() - 1].replace("''", "'"))
}

/// Collects every `#id` reference contained in an argument.
pub fn parse_references(value: &str) -> Vec<usize> {
    let chars = value.chars().collect::<Vec<_>>();
    let mut references = Vec::new();
    let mut cursor = 0;

    while cursor < chars.len() {
        if chars[cursor] != '#' {
            cursor += 1;
            continue;
        }

        cursor += 1;
        let start = cursor;
        while cursor < chars.len() && chars[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if start == cursor {
            continue;
        }
        if let Ok(reference) = chars[start..cursor]
            .iter()
            .collect::<String>()
            .parse::<usize>()
        {
            references.push(reference);
        }
    }

    references
}

/// Parses an optional unsigned integer value.
pub fn parse_optional_usize(value: &str) -> Option<usize> {
    value.trim().parse::<usize>().ok()
}

/// Parses a flattened list of 3D vectors.
pub fn parse_vec3_list(value: &str) -> Result<Vec<[f32; 3]>, String> {
    let numbers = parse_float_list(value);
    if !numbers.len().is_multiple_of(3) {
        return Err(format!(
            "coordinate component count {} is not divisible by 3",
            numbers.len()
        ));
    }
    Ok(numbers
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect())
}

/// Parses nested STEP integer lists.
pub fn parse_nested_integer_lists(value: &str) -> Vec<Vec<i64>> {
    let stripped = strip_outer_parentheses(value.trim());
    split_top_level_args(stripped)
        .into_iter()
        .map(parse_integer_list)
        .filter(|list| !list.is_empty())
        .collect()
}

/// Parses every signed integer contained in an argument.
pub fn parse_integer_list(value: &str) -> Vec<i64> {
    let mut values = Vec::new();
    let chars = value.chars().collect::<Vec<_>>();
    let mut cursor = 0;

    while cursor < chars.len() {
        if chars[cursor].is_ascii_digit()
            || (matches!(chars[cursor], '+' | '-')
                && chars
                    .get(cursor + 1)
                    .map(|next| next.is_ascii_digit())
                    .unwrap_or(false))
        {
            let start = cursor;
            cursor += 1;
            while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if let Ok(value) = chars[start..cursor]
                .iter()
                .collect::<String>()
                .parse::<i64>()
            {
                values.push(value);
            }
        } else {
            cursor += 1;
        }
    }

    values
}

/// Parses one required floating-point value with an entity-aware error.
pub fn parse_required_float(
    value: &str,
    record_id: usize,
    label: &str,
) -> Result<f32, ImportError> {
    let values = parse_float_list(value);
    values.first().copied().ok_or_else(|| {
        ImportError::InvalidData(format!("#{record_id} has invalid {label} numeric value"))
    })
}

/// Parses every STEP real number contained in an argument, including D exponents.
pub fn parse_float_list(value: &str) -> Vec<f32> {
    let mut values = Vec::new();
    let chars = value.chars().collect::<Vec<_>>();
    let mut cursor = 0;

    while cursor < chars.len() {
        if starts_number(&chars, cursor) {
            let start = cursor;
            cursor += 1;
            while cursor < chars.len() && is_number_char(chars[cursor]) {
                cursor += 1;
            }
            let token = chars[start..cursor]
                .iter()
                .collect::<String>()
                .replace(['D', 'd'], "E");
            if let Ok(value) = token.parse::<f32>() {
                values.push(value);
            }
        } else {
            cursor += 1;
        }
    }

    values
}

fn strip_step_comments(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len()
                && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
            {
                index += 1;
            }
            index = (index + 2).min(chars.len());
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }

    output
}

fn read_parenthesized(chars: &[char], start: usize) -> Result<(String, usize), ImportError> {
    let mut cursor = start;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut output = String::new();

    while cursor < chars.len() {
        let character = chars[cursor];

        if character == '\'' {
            output.push(character);
            if in_string && chars.get(cursor + 1) == Some(&'\'') {
                cursor += 1;
                output.push(chars[cursor]);
            } else {
                in_string = !in_string;
            }
            cursor += 1;
            continue;
        }

        if !in_string {
            match character {
                '(' => {
                    depth += 1;
                    if depth > 1 {
                        output.push(character);
                    }
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok((output, cursor + 1));
                    }
                    output.push(character);
                }
                _ => output.push(character),
            }
        } else {
            output.push(character);
        }

        cursor += 1;
    }

    Err(ImportError::InvalidData(
        "unterminated STEP parenthesized record".to_string(),
    ))
}

fn strip_outer_parentheses(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn starts_number(chars: &[char], cursor: usize) -> bool {
    chars[cursor].is_ascii_digit()
        || chars[cursor] == '.'
        || (matches!(chars[cursor], '+' | '-')
            && chars
                .get(cursor + 1)
                .map(|next| next.is_ascii_digit() || *next == '.')
                .unwrap_or(false))
}

fn is_number_char(character: char) -> bool {
    character.is_ascii_digit() || matches!(character, '+' | '-' | '.' | 'e' | 'E' | 'd' | 'D')
}

fn skip_whitespace(chars: &[char], cursor: &mut usize) {
    while *cursor < chars.len() && chars[*cursor].is_whitespace() {
        *cursor += 1;
    }
}
