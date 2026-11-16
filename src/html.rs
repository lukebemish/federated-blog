use lol_html::{RewriteStrSettings, doc_comments, doctype, element, html_content::ContentType, rewrite_str};
use regex::{Captures, Regex};

use crate::Error;

fn rewriter_settings<'a>() -> RewriteStrSettings<'a, 'a> {
    let block_tags = vec![
        "p", "div", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6", 
        "tr", "blockquote", "section", "article", "header", "footer"
    ].join(", ");
    let ignore_tags = vec!["script", "style", "head"].join(", ");

    RewriteStrSettings::new().append_element_content_handler(
        element!("pre", |el| {
            el.after("\n", ContentType::Text);
            let mut attrs = vec![];
            for ele in el.attributes() {
                attrs.push(ele.name());
            }
            for ele in attrs {
                el.remove_attribute(&ele);
            }
            Ok(())
        })
    ).append_element_content_handler(
        element!("svg, img, picture", |el| {
            el.replace(&format!("[{}]", el.tag_name()), ContentType::Text);
            el.remove();
            Ok(())
        })
    ).append_element_content_handler(
        element!(ignore_tags, |el| {
            el.remove();
            Ok(())
        })
    ).append_element_content_handler(
        element!(block_tags, |el| {
            el.append("\n", ContentType::Text);
            el.remove_and_keep_content();
            Ok(())
        })
    ).append_element_content_handler(
        element!("*:not(pre)", |el| {
            el.remove_and_keep_content();
            Ok(())
        }),
    ).append_document_content_handler(
        doc_comments!(|c| {
            c.remove();
            Ok(())
        })
    ).append_document_content_handler(
        doctype!(|d| {
            d.remove();
            Ok(())
        })
    )
}

pub(crate) fn strip_tags(html: &str) -> Result<String, Error> {
    let rewritten = rewrite_str(
        &html,
        rewriter_settings()
    )?;
    let mut pres = vec![];

    let pre_pattern = Regex::new("<\\s*pre\\s*>([\\s\\S]*?)</\\s*pre\\s*>")?;
    let rewritten = pre_pattern.replace_all(&rewritten, |m: &Captures| {
        let contents = match m.get(1) {
            None => "[pre]".to_owned(),
            Some(content) => match rewrite_str(content.as_str(), rewriter_settings()) {
                Ok(rewritten) => rewritten,
                _ => "[pre]".to_owned()
            }
        };
        pres.push(contents);
        format!("<pre {}>", pres.len() - 1)
    });

    let rewritten = rewritten.split('\n')
        .map(|line| {
            line.trim()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let pre_holder_pattern = Regex::new("<pre (\\d+)>")?;
    let rewritten = pre_holder_pattern.replace_all(&rewritten, |m: &Captures| {
        match m.get(1) {
            None => "[pre]".to_owned(),
            Some(index) => match index.as_str().parse::<usize>() {
                Ok(idx) => match pres.get(idx) {
                    Some(s) => s.to_owned(),
                    _ => "[pre]".to_owned()
                },
                _ => "[pre]".to_owned()
            }
        }
    });
    let rewritten = html_escape::decode_html_entities(&rewritten);
    Ok(rewritten.into_owned())
}
