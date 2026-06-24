//! Feather Lite cache decoder.
//!
//! The cache format is a small text contract used for tests, local converters,
//! and private CAD containers that expose pre-tessellated visualization data.
//! Native CATIA/NX cache decoders can later map their binary payloads into the
//! same document model without changing downstream mesh/export code.

use std::path::Path;

use crate::document::{LiteDocument, LiteMaterial, LiteMesh, LiteNode, LitePrimitive};
use crate::importer::ImportError;

/// Start marker for an embedded Feather Lite cache payload.
pub const CACHE_MARKER: &str = "FEATHER_CAD_LITE_CACHE_V1";

/// End marker for an embedded Feather Lite cache payload.
pub const CACHE_END_MARKER: &str = "END_FEATHER_CAD_LITE_CACHE";

/// Returns true when bytes contain an embedded cache marker.
pub fn contains_cache(bytes: &[u8]) -> bool {
    find_bytes(bytes, CACHE_MARKER.as_bytes()).is_some()
}

/// Borrowed Feather cache payload with its byte range in the source buffer.
#[derive(Debug, Clone, Copy)]
pub struct CachePayload<'a> {
    pub start: usize,
    pub end: usize,
    pub text: &'a str,
}

/// Extracts a standalone or embedded cache payload with byte range metadata.
pub fn extract_cache_payload(bytes: &[u8]) -> Result<Option<CachePayload<'_>>, ImportError> {
    let Some(start) = find_bytes(bytes, CACHE_MARKER.as_bytes()) else {
        return Ok(None);
    };

    let end_marker = CACHE_END_MARKER.as_bytes();
    let after_start = &bytes[start..];
    let end = find_bytes(after_start, end_marker)
        .map(|relative_end| start + relative_end + end_marker.len())
        .unwrap_or(bytes.len());

    let text = std::str::from_utf8(&bytes[start..end]).map_err(|error| {
        ImportError::InvalidData(format!(
            "embedded Feather Lite cache is not valid UTF-8: {error}"
        ))
    })?;

    Ok(Some(CachePayload { start, end, text }))
}

/// Extracts the cache text from a standalone or embedded payload.
pub fn extract_cache_text(bytes: &[u8]) -> Result<Option<String>, ImportError> {
    Ok(extract_cache_payload(bytes)?.map(|payload| payload.text.to_string()))
}

/// Decodes Feather Lite cache text into the visual IR.
pub fn decode_cache_text(
    text: &str,
    source_format: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let mut parser = CacheParser::new(source_format, source_path);
    parser.parse(text)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }

    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

struct CacheParser<'a> {
    document: LiteDocument,
    current_mesh: Option<LiteMesh>,
    current_primitive: Option<LitePrimitive>,
    source_path: Option<&'a Path>,
}

impl<'a> CacheParser<'a> {
    fn new(source_format: &str, source_path: Option<&'a Path>) -> Self {
        Self {
            document: LiteDocument::new(source_format, "cache-only"),
            current_mesh: None,
            current_primitive: None,
            source_path,
        }
    }

    fn parse(&mut self, text: &str) -> Result<LiteDocument, ImportError> {
        if let Some(path) = self.source_path {
            self.document.metadata.source_path = Some(path.display().to_string());
        }

        for (line_index, raw_line) in text.lines().enumerate() {
            let tokens = tokenize_cache_line(raw_line, line_index)?;
            if tokens.is_empty() {
                continue;
            }
            let command = tokens[0].as_str();

            match command {
                CACHE_MARKER => {}
                CACHE_END_MARKER => break,
                "document" => {
                    if tokens.len() > 1 {
                        self.document
                            .metadata
                            .warnings
                            .push(format!("document name: {}", tokens[1..].join(" ")));
                    }
                }
                "material" => self.parse_material(&tokens, line_index)?,
                "mesh" => self.start_mesh(&tokens, line_index)?,
                "primitive" => self.start_primitive(&tokens, line_index)?,
                "v" => self.parse_vertex(&tokens, line_index)?,
                "tri" => self.parse_triangle(&tokens, line_index)?,
                "endprimitive" => self.end_primitive(line_index)?,
                "endmesh" => self.end_mesh(line_index)?,
                "node" => self.parse_node(&tokens, line_index)?,
                "reference" => self.parse_reference(&tokens, line_index)?,
                _ => {
                    return Err(ImportError::InvalidData(format!(
                        "line {}: unknown cache command `{command}`",
                        line_index + 1
                    )));
                }
            }
        }

        if self.current_primitive.is_some() {
            return Err(ImportError::InvalidData(
                "cache ended before endprimitive".to_string(),
            ));
        }

        if self.current_mesh.is_some() {
            return Err(ImportError::InvalidData(
                "cache ended before endmesh".to_string(),
            ));
        }

        self.document.add_default_nodes_for_unreferenced_meshes();
        self.document.refresh_metadata();
        Ok(std::mem::replace(
            &mut self.document,
            LiteDocument::new("Unknown", "cache-only"),
        ))
    }

    fn parse_material(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 6 {
            return Err(line_error(
                line_index,
                "material expects: material <name> <r> <g> <b> <a>",
            ));
        }

        let color = [
            parse_f32(tokens[2].as_str(), line_index, "material r")?,
            parse_f32(tokens[3].as_str(), line_index, "material g")?,
            parse_f32(tokens[4].as_str(), line_index, "material b")?,
            parse_f32(tokens[5].as_str(), line_index, "material a")?,
        ];
        self.document
            .materials
            .push(LiteMaterial::new(tokens[1].as_str(), color));
        Ok(())
    }

    fn start_mesh(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 2 {
            return Err(line_error(line_index, "mesh expects: mesh <name>"));
        }
        if self.current_mesh.is_some() {
            return Err(line_error(line_index, "nested mesh blocks are not allowed"));
        }
        self.current_mesh = Some(LiteMesh::new(tokens[1].as_str()));
        Ok(())
    }

    fn start_primitive(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 2 {
            return Err(line_error(
                line_index,
                "primitive expects: primitive <material-index|none>",
            ));
        }
        if self.current_mesh.is_none() {
            return Err(line_error(line_index, "primitive must be inside a mesh"));
        }
        if self.current_primitive.is_some() {
            return Err(line_error(
                line_index,
                "nested primitive blocks are not allowed",
            ));
        }

        let material = if tokens[1].eq_ignore_ascii_case("none") {
            None
        } else {
            Some(parse_usize(
                tokens[1].as_str(),
                line_index,
                "material index",
            )?)
        };

        self.current_primitive = Some(LitePrimitive::new(material));
        Ok(())
    }

    fn parse_vertex(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 4 && tokens.len() != 7 {
            return Err(line_error(
                line_index,
                "v expects: v <x> <y> <z> [<nx> <ny> <nz>]",
            ));
        }
        let Some(primitive) = self.current_primitive.as_mut() else {
            return Err(line_error(line_index, "vertex must be inside a primitive"));
        };

        primitive.positions.push([
            parse_f32(tokens[1].as_str(), line_index, "x")?,
            parse_f32(tokens[2].as_str(), line_index, "y")?,
            parse_f32(tokens[3].as_str(), line_index, "z")?,
        ]);

        if tokens.len() == 7 {
            primitive.normals.push([
                parse_f32(tokens[4].as_str(), line_index, "nx")?,
                parse_f32(tokens[5].as_str(), line_index, "ny")?,
                parse_f32(tokens[6].as_str(), line_index, "nz")?,
            ]);
        }

        Ok(())
    }

    fn parse_triangle(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 4 {
            return Err(line_error(line_index, "tri expects: tri <a> <b> <c>"));
        }
        let Some(primitive) = self.current_primitive.as_mut() else {
            return Err(line_error(
                line_index,
                "triangle must be inside a primitive",
            ));
        };

        primitive.indices.push(parse_u32(
            tokens[1].as_str(),
            line_index,
            "triangle index a",
        )?);
        primitive.indices.push(parse_u32(
            tokens[2].as_str(),
            line_index,
            "triangle index b",
        )?);
        primitive.indices.push(parse_u32(
            tokens[3].as_str(),
            line_index,
            "triangle index c",
        )?);
        Ok(())
    }

    fn end_primitive(&mut self, line_index: usize) -> Result<(), ImportError> {
        let Some(primitive) = self.current_primitive.take() else {
            return Err(line_error(
                line_index,
                "endprimitive without primitive block",
            ));
        };
        let Some(mesh) = self.current_mesh.as_mut() else {
            return Err(line_error(line_index, "endprimitive without mesh block"));
        };
        mesh.primitives.push(primitive);
        Ok(())
    }

    fn end_mesh(&mut self, line_index: usize) -> Result<(), ImportError> {
        if self.current_primitive.is_some() {
            return Err(line_error(
                line_index,
                "endmesh before closing current primitive",
            ));
        }
        let Some(mut mesh) = self.current_mesh.take() else {
            return Err(line_error(line_index, "endmesh without mesh block"));
        };
        mesh.recompute_bbox();
        self.document.meshes.push(mesh);
        Ok(())
    }

    fn parse_node(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 4 && tokens.len() != 20 {
            return Err(line_error(
                line_index,
                "node expects: node <name> <mesh-index|none> <parent-index|root> [16 matrix values]",
            ));
        }

        let mesh = if tokens[2].eq_ignore_ascii_case("none") {
            None
        } else {
            Some(parse_usize(
                tokens[2].as_str(),
                line_index,
                "node mesh index",
            )?)
        };

        let parent = if tokens[3].eq_ignore_ascii_case("root") {
            None
        } else {
            Some(parse_usize(
                tokens[3].as_str(),
                line_index,
                "node parent index",
            )?)
        };

        let mut node = LiteNode::new(tokens[1].as_str(), mesh);
        if tokens.len() == 20 {
            for matrix_index in 0..16 {
                let column = matrix_index / 4;
                let row = matrix_index % 4;
                node.transform[column][row] = parse_f32(
                    tokens[4 + matrix_index].as_str(),
                    line_index,
                    "matrix value",
                )?;
            }
        }

        let node_index = self.document.nodes.len();
        self.document.nodes.push(node);

        if let Some(parent_index) = parent {
            let Some(parent_node) = self.document.nodes.get_mut(parent_index) else {
                return Err(line_error(line_index, "node parent index is out of range"));
            };
            parent_node.children.push(node_index);
        }

        Ok(())
    }

    fn parse_reference(&mut self, tokens: &[String], line_index: usize) -> Result<(), ImportError> {
        if tokens.len() != 4 && tokens.len() != 20 {
            return Err(line_error(
                line_index,
                "reference expects: reference <name> <path> <parent-index|root> [16 matrix values]",
            ));
        }

        let parent = if tokens[3].eq_ignore_ascii_case("root") {
            None
        } else {
            Some(parse_usize(
                tokens[3].as_str(),
                line_index,
                "reference parent index",
            )?)
        };

        let mut node = LiteNode::new(tokens[1].as_str(), None);
        node.source_id = Some(tokens[2].to_string());
        if tokens.len() == 20 {
            for matrix_index in 0..16 {
                let column = matrix_index / 4;
                let row = matrix_index % 4;
                node.transform[column][row] = parse_f32(
                    tokens[4 + matrix_index].as_str(),
                    line_index,
                    "matrix value",
                )?;
            }
        }

        let node_index = self.document.nodes.len();
        self.document.nodes.push(node);

        if let Some(parent_index) = parent {
            let Some(parent_node) = self.document.nodes.get_mut(parent_index) else {
                return Err(line_error(
                    line_index,
                    "reference parent index is out of range",
                ));
            };
            parent_node.children.push(node_index);
        }

        Ok(())
    }
}

fn tokenize_cache_line(line: &str, line_index: usize) -> Result<Vec<String>, ImportError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quote = None::<char>;
    let mut token_started = false;

    while let Some(character) = chars.next() {
        if let Some(quote_char) = quote {
            if character == quote_char {
                quote = None;
                token_started = true;
                continue;
            }
            if character == '\\'
                && let Some(next) = chars.peek().copied()
                && (next == quote_char || next == '\\')
            {
                current.push(next);
                chars.next();
                token_started = true;
                continue;
            }
            current.push(character);
            token_started = true;
            continue;
        }

        if character == '#' {
            break;
        }
        if character.is_whitespace() {
            if token_started {
                tokens.push(std::mem::take(&mut current));
                token_started = false;
            }
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
            token_started = true;
            continue;
        }

        current.push(character);
        token_started = true;
    }

    if let Some(quote_char) = quote {
        return Err(line_error(
            line_index,
            &format!("unterminated quoted token starting with `{quote_char}`"),
        ));
    }
    if token_started {
        tokens.push(current);
    }

    Ok(tokens)
}

fn parse_f32(token: &str, line_index: usize, field: &str) -> Result<f32, ImportError> {
    token
        .parse::<f32>()
        .map_err(|error| line_error(line_index, &format!("invalid {field}: {error}")))
}

fn parse_u32(token: &str, line_index: usize, field: &str) -> Result<u32, ImportError> {
    token
        .parse::<u32>()
        .map_err(|error| line_error(line_index, &format!("invalid {field}: {error}")))
}

fn parse_usize(token: &str, line_index: usize, field: &str) -> Result<usize, ImportError> {
    token
        .parse::<usize>()
        .map_err(|error| line_error(line_index, &format!("invalid {field}: {error}")))
}

fn line_error(line_index: usize, message: &str) -> ImportError {
    ImportError::InvalidData(format!("line {}: {message}", line_index + 1))
}
