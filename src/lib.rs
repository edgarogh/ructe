#[macro_use]
extern crate nom;

use nom::{alpha, alphanumeric, multispace, eof};
use nom::IResult::*;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::str::from_utf8;

#[derive(Debug, PartialEq, Eq)]
struct Template {
    preamble: Vec<String>,
    args: Vec<String>,
    body: Vec<TemplateExpression>,
}

impl Template {
    fn write_rust(&self, out: &mut Write, name: &str) -> io::Result<()> {
        write!(out,
               "{preamble}\n\
                pub fn {name}(out: &mut Write{args}) -> io::Result<()> {{\n\
                {body}\
                Ok(())\n\
                }}",
               preamble = self.preamble
                   .iter()
                   .map(|l| format!("{};\n", l))
                   .collect::<String>(),
               name = name,
               args = self.args
                   .iter()
                   .map(|a| format!(", {}", a))
                   .collect::<String>(),
               body = self.body
                   .iter()
                   .map(|b| b.code())
                   .collect::<String>())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TemplateExpression {
    Comment,
    Text { text: String },
    Expression { expr: String },
}

impl TemplateExpression {
    fn code(&self) -> String {
        match *self {
            TemplateExpression::Comment => String::new(),
            TemplateExpression::Text { ref text } => {
                format!("try!(write!(out, \"{}\"));\n", text)
            }
            TemplateExpression::Expression { ref expr } => {
                format!("try!({}.to_html(out));\n", expr)
            }
        }
    }
}

named!(template<&[u8], Template>,
       chain!(
           spacelike ~
           preamble: many0!(chain!(tag!("@") ~
                                   code: is_not!(";()") ~
                                   tag!(";") ~
                                   spacelike,
                                   ||from_utf8(code).unwrap().to_string()
                                   )) ~
           tag!("@(") ~
           args: separated_list!(tag!(", "), formal_argument) ~
           tag!(")") ~
           spacelike ~
           body: many0!(template_expression) ~
           eof,
           || { Template { preamble: preamble, args: args, body: body } }
           )
);

// TODO Actually parse arguments!
named!(formal_argument<&[u8], String>,
       chain!(
           raw: is_not!(",)"),
           || from_utf8(raw).unwrap().to_string()
               )
       );

named!(template_expression<&[u8], TemplateExpression>,
       alt!(
           chain!(
               comment,
               || TemplateExpression::Comment
               ) |
           chain!(
               text: is_not!("@"),
               || TemplateExpression::Text {
                   text: from_utf8(text).unwrap().to_string()
               }) |
           chain!(
               tag!("@") ~
               expr: expression,
               || TemplateExpression::Expression{ expr: expr }
           )
       )
);


named!(expression<&[u8], String>,
       alt!(
           chain!(pre: rust_name ~
                  char!('.') ~
                  post: expression,
                  || format!("{}.{}", pre, post)) |
           rust_name
               ));

#[test]
fn test_expression() {
    // Proper expressions, each followed by two non-expression characters.
    for input in &[&b"foo  "[..],
                   &b"foo<x"[..],
                   &b"foo!!"[..],
                   &b"x15  "[..],
                   &b"foo. "[..],
                   &b"foo.bar  "[..],
                   &b"boo.bar.baz##"[..]] {
        let i = input.len() - 2;
        assert_eq!(expression(*input),
                   Done(&input[i..],
                        from_utf8(&input[..i]).unwrap().to_string()));
    }
    // non-expressions
    for input in &[&b".foo"[..], &b" foo"[..], &b"()"[..]] {
        assert_eq!(expression(*input),
                   Error(nom::Err::Position(nom::ErrorKind::Alt, &input[..])));
    }
}

named!(rust_name<&[u8], String>,
       chain!(first: alpha ~
              rest: opt!(alphanumeric),
              || format!("{}{}",
                         from_utf8(first).unwrap(),
                         from_utf8(rest.unwrap_or(b"")).unwrap())));

named!(spacelike<&[u8], ()>,
       chain!(many0!(alt!(
           comment |
           chain!(multispace, ||()))),
              || ()));

named!(comment<&[u8], ()>,
       value!((), delimited!(tag!("@*"),
                             many0!(alt!(
                                 chain!(is_not!("*"), ||()) |
                                 chain!(tag!("*") ~ none_of!("@"), ||())
                                     )),
                             tag!("*@"))));

#[test]
fn test_comment() {
    assert_eq!(comment(b"@* a simple comment *@"), Done(&b""[..], ()));
}
#[test]
fn test_comment2() {
    assert_eq!(comment(b" @* comment *@"),
               Error(nom::Err::Position(nom::ErrorKind::Tag,
                                        &b" @* comment *@"[..])));
}
#[test]
fn test_comment3() {
    assert_eq!(comment(b"@* comment *@ & stuff"), Done(&b" & stuff"[..], ()));
}
#[test]
fn test_comment4() {
    assert_eq!(comment(b"@* comment *@ and @* another *@"),
               Done(&b" and @* another *@"[..], ()));
}
#[test]
fn test_comment5() {
    assert_eq!(comment(b"@* comment containing * and @ *@"),
               Done(&b""[..], ()));
}
#[test]
fn test_comment6() {
    assert_eq!(comment(b"@*** peculiar comment ***@***"),
               Done(&b"***"[..], ()));
}

pub fn compile_templates(indir: &Path,
                         outdir: &Path,
                         names: &[&str])
                         -> io::Result<()> {
    File::create(outdir.join("templates.rs")).and_then(|mut f| {
        try!(write!(f, "mod templates {{\n\
                        use std::io::{{self, Write}};\n\
                        use std::fmt::Display;\n"));
        for name in names {
            let path = indir.join(format!("{}.rs.html", name));
            println!("cargo:rerun-if-changed={}", path.to_string_lossy());
            let mut input = try!(File::open(&path));
            let mut buf = Vec::new();
            try!(input.read_to_end(&mut buf));
            match template(&buf) {
                Done(_, t) => try!(t.write_rust(&mut f, name)),
                Error(err) => {
                    println!("cargo:warning=Template parse error in {:?}: {}",
                             path, err)
                }
                Incomplete(needed) => {
                    println!("cargo:warning=\
                              Failed to parse template {:?}: {:?} needed",
                             path, needed)
                }
            }
        }
        write!(f, "{}\n}}\n", include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
                                                   "/src/template_utils.rs")))
    })
}

mod foo {
    use std::fmt::Display;
    use std::io::{self, Write};
    include!("template_utils.rs");

    #[test]
    fn test_encoded() {
        let mut buf = Vec::new();
        "a < b".to_html(&mut buf).unwrap();
        assert_eq!(b"a &lt; b", &buf[..]);
    }
    #[test]
    fn test_raw_html() {
        let mut buf = Vec::new();
        Html("a<b>c</b>").to_html(&mut buf).unwrap();
        assert_eq!(b"a<b>c</b>", &buf[..]);
    }
}