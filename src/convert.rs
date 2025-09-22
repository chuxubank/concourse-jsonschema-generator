use std::collections::HashMap;

use itertools::Itertools;

use crate::lit::types::{LitDocument, LitNode};
use crate::schema::types::{Property, PropertyType, Schema};

pub fn to_jsonschemas(doc: &LitDocument) -> Vec<Schema> {
  collect_schemas(doc)
}

fn extend_child_properties(
  child_schemas: &mut Vec<Schema>,
  attributes: &HashMap<String, Property>,
) {
  if attributes.len() > 0 {
    for child in child_schemas {
      if !child.is_group_member {
        continue;
      }
      let child_props = &mut child.properties;
      child_props.extend(attributes.clone().into_iter());
    }
  }
}

fn collect_schemas(doc: &LitDocument) -> Vec<Schema> {
  doc
    .iter()
    .flat_map(|node| match node {
      LitNode::Text(_) => vec![],

      LitNode::Fn(schema, args) if (schema == "schema") || (schema == "schema-group") => {
        let mut found_schemas: Vec<Schema> = vec![];

        let schema_name = text_to_markdown(&args[0])
          .trim()
          .replace("`", "_")
          .replace("-", "_")
          .replace(" ", "_")
          .replace("__", "_")
          .trim_start_matches("_")
          .to_string();

        log::debug!("In schema {}", schema_name);

        let (attrs_vec, schemas_vecvec): (Vec<_>, Vec<_>) =
          collect_attributes(if schema == "schema" {
            &args[1]
          } else {
            &args[2]
          })
          .into_iter()
          .unzip();

        let attributes = attrs_vec.into_iter().collect::<HashMap<String, Property>>();

        let inner_schemas = schemas_vecvec.into_iter().flat_map(|svv| svv).collect_vec();
        let mut child_schemas = args.into_iter().flat_map(collect_schemas).collect_vec();

        extend_child_properties(&mut child_schemas, &attributes);

        log::debug!("Out of schema {}", schema_name);

        let group_members = child_schemas
          .iter()
          .filter(|s| s.is_group_member)
          .map(|s| s.schema_name.clone())
          .collect_vec();

        let has_group_memberes = group_members.len() > 0;

        found_schemas.push(Schema {
          is_group_member: schema == "schema-group",
          group_members: group_members,
          schema_name: schema_name,
          properties: if has_group_memberes {
            HashMap::new()
          } else {
            attributes
          },
        });

        found_schemas.extend(inner_schemas);
        found_schemas.extend(child_schemas);

        found_schemas
      }
      // Do not collect schemas from props. Not your job.
      LitNode::Fn(prop, _) if prop == "required-attribute" || prop == "optional-attribute" => {
        vec![]
      }
      LitNode::Fn(_other_fn, args) => args.into_iter().flat_map(collect_schemas).collect(),
      LitNode::Comment(_) => vec![],
    })
    .collect()
}

fn collect_attributes(doc: &LitDocument) -> Vec<((String, Property), Vec<Schema>)> {
  doc
    .iter()
    .flat_map(|node| match node {
      LitNode::Text(_) => vec![],

      LitNode::Fn(attribute_type, args)
        if (attribute_type == "required-attribute" || attribute_type == "optional-attribute") =>
      {
        let prop_value = convert_prop(&args, attribute_type);
        let inner_schemas: Vec<_> = args.iter().flat_map(collect_schemas).collect();
        vec![(prop_value, inner_schemas)]
      }

      LitNode::Fn(other_fn, args) if (other_fn != "schema" && other_fn != "schema-group") => {
        args.iter().flat_map(collect_attributes).collect::<Vec<_>>()
      }

      _ => vec![],
    })
    .collect()
}

fn convert_prop(args: &Vec<Vec<LitNode>>, attribute_type: &String) -> (String, Property) {
  let prop_name = text_to_markdown(&args[0]).trim().to_string();
  log::debug!("- In prop {}", prop_name);

  let type_name = text_to_markdown(&args[1]).trim().to_string();

  let is_list = type_name.starts_with("[");

  let documentation = &args[2];

  log::debug!("- Out prop {}", prop_name);

  (
    prop_name,
    Property {
      required: attribute_type == "required-attribute",
      docs: text_to_markdown(documentation).trim().to_string(),
      type_name: parse_type(&type_name.replace("-", "_")),
      list: is_list,
    },
  )
}

peg::parser! {
  grammar lit_type_parser() for str {

    pub rule lit_type() -> PropertyType
      = union_type() / non_union_type()

    rule non_union_type() -> PropertyType
      = array_type() / dictionary_type() / constant_type() / ref_type()

    rule array_type() -> PropertyType
      = "[" inner_type:lit_type() "]" { PropertyType::ArrayOf(Box::new(inner_type)) }

    rule union_type() -> PropertyType =
      inner_types:(non_union_type() ++ (_ "|" _)) { PropertyType::OneOf(inner_types) }

    rule _ = [' ' | '\n']*;

    rule key_or_value_string() -> String
      = name:$(['a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_']+) { String::from(name) }

    rule type_identifier() -> String
      = name:$(['a'..='z' | 'A'..='Z' | '_']+) { String::from(name) }

    rule dictionary_type() -> PropertyType
      = "{" _ key_or_value_string() _ ":" _ key_or_value_string() "}" { PropertyType::Dict }

    rule constant_type() -> PropertyType
      = "`" value:key_or_value_string() "`" { PropertyType::Constant(value) }

    rule ref_type() -> PropertyType
      = name:key_or_value_string() {
        PropertyType::Ref(
          if name.contains(".") { "string".to_string() } else { name }
        )
      }


  }
}

fn parse_type(s: &str) -> PropertyType {
  match lit_type_parser::lit_type(s) {
    Ok(res) => res,
    Err(e) => {
      eprintln!("Error parsing type: {}", s);
      eprintln!("{}", e);
      panic!("Unable to parse type")
    }
  }
}

pub fn text_to_markdown(nodes: &Vec<LitNode>) -> String {
  // Collect all mapped strings into a single String.
  let content: String = nodes
    .iter()
    .map(|n| match n {
      LitNode::Text(t) => clean_text(t),
      LitNode::Fn(name, args) => match name.as_str() {
        // Use a nested match to handle specific function names.
        "example-toggle" => {
          format!(
            "\n@example {}\n{}",
            text_to_markdown(&args[0]),
            text_to_markdown(&args[1])
          )
        }
        "reference" => {
          // Check the number of arguments
          match args.len() {
            // Case 1: \reference{link}{text}
            // This will be converted to a Markdown link: [text](#link)
            2 => {
              let link_target = text_to_markdown(&args[0]);
              let link_text = text_to_markdown(&args[1]);
              format!("[{}](#{})", link_text, link_target)
            }
            // Case 2: \reference{link}
            // This will be converted to a simple link: [link](#link)
            1 => {
              let link_target = text_to_markdown(&args[0]);
              format!("[{}](#{})", link_target, link_target)
            }
            // Case 3: Other number of arguments
            // This is an invalid number of arguments, so we can return a warning.
            _ => {
              log::warn!(
                "'reference' function expects 1 or 2 arguments, but got {}.",
                args.len()
              );
              // Returning an empty string is a safe default to avoid breaking the output.
              "".to_string()
            }
          }
        }
        "codeblock" => {
          // It's more idiomatic to use `trim_codeblock` on the raw text directly.
          format!("\n\n{}\n\n", trim_codeblock(&raw_text(&args[1])))
        }
        "code" => {
          format!("`{}`", raw_text(&args[0]))
        }
        "bold" => {
          format!("**{}**", text_to_markdown(&args[0]))
        }
        "warn" => {
          // This seems to be a pass-through function, so we just process its argument.
          text_to_markdown(&args[0])
        }
        // Handle all other functions by processing their arguments.
        _ => args.iter().map(text_to_markdown).collect::<String>(),
      },
      _ => "".to_string(),
    })
    .collect();

  // Perform replacements on the final collected string.
  content.replace("\\{", "{").replace("\\}", "}")
}

pub fn clean_text(text: &str) -> String {
  text
    .lines()
    // TODO: Do not trim beginning of first and end of last
    .map(|t| " ".to_string() + t.trim() + " ")
    .map(|t| if t == "" { "\n\n".to_string() } else { t })
    .collect()
}

pub fn trim_codeblock(text: &str) -> String {
  let trim_start_count = text
    .lines()
    .filter(|l| l.len() > 0)
    .map(|s| s.chars().position(|c| c != ' '))
    .filter(|x| x.is_some())
    .map(|x| x.unwrap())
    .min()
    .unwrap_or(0);

  text
    .split("\n")
    .map(|l| {
      if l.len() > trim_start_count {
        &l[trim_start_count..]
      } else {
        l.trim()
      }
    })
    .map(|l| format!("    {}", l))
    // .collect_v
    .join("\n")
    // .trim()
    .to_string()
}

pub fn raw_text(nodes: &Vec<LitNode>) -> String {
  nodes
    .iter()
    .map(|n| match n {
      LitNode::Text(t) => t.clone(),
      LitNode::Fn(_name, args) => args.iter().map(raw_text).collect(),
      _ => "".to_string(),
    })
    .collect::<String>()
}
