//! Page expressions for exporting a subset of the document.
//!
//! The reader writes what the print dialog accepts (`1,2,100,158`, `1-55`), and
//! this module turns it into page indices. It knows nothing about GTK.

/// Pages named by `expression`, zero-based, in the order written and without
/// duplicates. `total` is how many pages the document has.
///
/// Accepted forms, separated by `,` or `;`:
/// a number (`7`), a range in either direction (`3-9`, `9-3`), `pares`,
/// `impares`, `todas` and `última`. `all`, `even`, `odd` and `last` are
/// synonyms. The expression is what the reader typed, so the error messages
/// explain what is wrong instead of silently exporting nothing.
pub fn parse_pages(expression: &str, total: usize) -> Result<Vec<usize>, String> {
    if total == 0 {
        return Err("El documento no tiene páginas.".to_string());
    }
    if expression.trim().is_empty() {
        return Err("Escribí qué páginas querés exportar.".to_string());
    }

    let mut pages: Vec<usize> = Vec::new();
    let mut push = |page: usize| {
        if !pages.contains(&page) {
            pages.push(page);
        }
    };

    for token in expression.split([',', ';']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        match token.to_lowercase().as_str() {
            "todas" | "todos" | "all" => {
                for page in 0..total {
                    push(page);
                }
                continue;
            }
            // Page numbers are one-based: "pares" is 2, 4, 6…
            "pares" | "even" => {
                for page in (1..total).step_by(2) {
                    push(page);
                }
                continue;
            }
            "impares" | "odd" => {
                for page in (0..total).step_by(2) {
                    push(page);
                }
                continue;
            }
            "última" | "ultima" | "last" => {
                push(total - 1);
                continue;
            }
            _ => {}
        }

        if let Some((from, to)) = token.split_once('-') {
            let from = page_number(from, total)?;
            let to = page_number(to, total)?;
            if from <= to {
                for page in from..=to {
                    push(page);
                }
            } else {
                for page in (to..=from).rev() {
                    push(page);
                }
            }
            continue;
        }

        push(page_number(token, total)?);
    }

    if pages.is_empty() {
        return Err("La expresión no incluye ninguna página.".to_string());
    }
    Ok(pages)
}

/// One one-based page number from the expression, as a zero-based index.
fn page_number(text: &str, total: usize) -> Result<usize, String> {
    let text = text.trim();
    let Ok(number) = text.parse::<usize>() else {
        return Err(format!(
            "No entiendo «{text}»: usá números, rangos (3-9), «pares», «impares», «última» o «todas»."
        ));
    };
    if number == 0 || number > total {
        return Err(format!(
            "La página {number} no existe: el documento tiene {total}."
        ));
    }
    Ok(number - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_keeps_the_order_written() {
        assert_eq!(parse_pages("1,2,100,158", 666).unwrap(), vec![0, 1, 99, 157]);
        assert_eq!(parse_pages("7", 10).unwrap(), vec![6]);
    }

    #[test]
    fn ranges_expand_in_either_direction() {
        assert_eq!(parse_pages("1-5", 10).unwrap(), vec![0, 1, 2, 3, 4]);
        assert_eq!(parse_pages("5-1", 10).unwrap(), vec![4, 3, 2, 1, 0]);
        assert_eq!(parse_pages("3-3", 10).unwrap(), vec![2]);
    }

    #[test]
    fn separators_and_spaces_are_flexible() {
        assert_eq!(parse_pages(" 2 ; 4 ,6 ", 10).unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_pages("1,,2", 10).unwrap(), vec![0, 1]);
    }

    #[test]
    fn duplicates_collapse_to_the_first_mention() {
        assert_eq!(parse_pages("1,1,2,1-2", 10).unwrap(), vec![0, 1]);
    }

    #[test]
    fn keywords_cover_whole_documents_and_parity() {
        assert_eq!(parse_pages("todas", 4).unwrap(), vec![0, 1, 2, 3]);
        assert_eq!(parse_pages("pares", 6).unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_pages("impares", 6).unwrap(), vec![0, 2, 4]);
        assert_eq!(parse_pages("última", 6).unwrap(), vec![5]);
        assert_eq!(parse_pages("ALL", 3).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn out_of_range_pages_are_reported() {
        assert_eq!(
            parse_pages("0", 10).unwrap_err(),
            "La página 0 no existe: el documento tiene 10."
        );
        assert_eq!(
            parse_pages("11", 10).unwrap_err(),
            "La página 11 no existe: el documento tiene 10."
        );
        assert!(parse_pages("1-12", 10).is_err(), "el rango también se valida");
    }

    #[test]
    fn garbage_names_what_it_could_not_read() {
        let error = parse_pages("1, abc", 10).unwrap_err();
        assert!(error.contains("abc"), "{error}");
        assert!(error.contains("3-9"), "{error}");
    }

    #[test]
    fn an_empty_expression_asks_for_one() {
        assert_eq!(
            parse_pages("   ", 10).unwrap_err(),
            "Escribí qué páginas querés exportar."
        );
        assert!(
            parse_pages("1-", 10).unwrap_err().starts_with("No entiendo"),
            "un rango incompleto se reporta como número ilegible"
        );
        assert!(parse_pages("", 10).is_err());
    }

    #[test]
    fn a_document_without_pages_is_rejected() {
        assert_eq!(
            parse_pages("1", 0).unwrap_err(),
            "El documento no tiene páginas."
        );
    }
}
