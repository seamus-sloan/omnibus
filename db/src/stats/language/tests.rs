use super::*;

// Regression for #2466: `en 19, en-US 7, eng 1` was three languages of one
// 28-book library.
#[test]
fn language_label_folds_every_spelling_of_english_onto_one_bucket() {
    assert_eq!(language_label("en"), "English");
    assert_eq!(language_label("en-US"), "English");
    assert_eq!(language_label("en_GB"), "English");
    assert_eq!(language_label("eng"), "English");
    assert_eq!(language_label(" EN "), "English");
}

#[test]
fn language_label_folds_the_bibliographic_and_terminological_three_letter_forms() {
    assert_eq!(language_label("ger"), "German");
    assert_eq!(language_label("deu"), "German");
    assert_eq!(language_label("de"), "German");
    assert_eq!(language_label("fre"), "French");
    assert_eq!(language_label("fra"), "French");
}

#[test]
fn language_label_names_every_way_a_file_declines_to_answer_unknown() {
    assert_eq!(language_label("und"), UNKNOWN_LABEL);
    assert_eq!(language_label("UND"), UNKNOWN_LABEL);
    assert_eq!(language_label(""), UNKNOWN_LABEL);
    assert_eq!(language_label("   "), UNKNOWN_LABEL);
    assert_eq!(language_label("mul"), UNKNOWN_LABEL);
    assert_eq!(language_label("zxx"), UNKNOWN_LABEL);
}

#[test]
fn language_label_keeps_an_unnamed_code_rather_than_guessing_at_it() {
    // No name to give it, so it keeps its own subtag instead of folding into
    // a bucket that would claim to know what it is.
    assert_eq!(language_label("qu"), "QU");
    assert_eq!(language_label("qu-BO"), "QU");
}

#[test]
fn language_label_drops_script_and_region_from_a_bcp47_tag() {
    assert_eq!(language_label("zh-Hant"), "Chinese");
    assert_eq!(language_label("pt_BR"), "Portuguese");
}
