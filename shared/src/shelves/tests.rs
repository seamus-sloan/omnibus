use super::*;

#[test]
fn wire_tokens_round_trip_for_every_enum() {
    for f in [
        RuleField::Tag,
        RuleField::Author,
        RuleField::Series,
        RuleField::Rating,
        RuleField::Status,
        RuleField::Format,
        RuleField::Year,
        RuleField::DateAdded,
        RuleField::DateUpdated,
    ] {
        assert_eq!(RuleField::from_str(f.as_str()), Some(f));
    }
    for op in [
        RuleOp::Is,
        RuleOp::IsNot,
        RuleOp::Contains,
        RuleOp::StartsWith,
        RuleOp::Gte,
        RuleOp::Includes,
        RuleOp::InLast,
        RuleOp::Between,
        RuleOp::Before,
        RuleOp::After,
    ] {
        assert_eq!(RuleOp::from_str(op.as_str()), Some(op));
    }
    assert_eq!(ShelfKind::from_str("manual"), Some(ShelfKind::Manual));
    assert_eq!(Visibility::from_str("public"), Some(Visibility::Public));
    assert_eq!(MatchMode::from_str("all"), Some(MatchMode::All));
    assert_eq!(RuleField::from_str("bogus"), None);
}

#[test]
fn accepts_gates_ops_per_field() {
    assert!(RuleField::Tag.accepts(RuleOp::IsNot));
    assert!(!RuleField::Tag.accepts(RuleOp::Gte));
    assert!(RuleField::Format.accepts(RuleOp::Includes));
    // Author/series are name-based text fields: they take the same text ops as
    // tag (is / is not / contains / starts with) but not the numeric ones.
    assert!(RuleField::Author.accepts(RuleOp::IsNot));
    assert!(RuleField::Author.accepts(RuleOp::Contains));
    assert!(RuleField::Series.accepts(RuleOp::StartsWith));
    assert!(!RuleField::Author.accepts(RuleOp::Gte));
    assert!(RuleField::DateAdded.accepts(RuleOp::Between));
    assert!(!RuleField::DateAdded.accepts(RuleOp::Is));
}

#[test]
fn rule_validate_rejects_bad_op_and_empty_value() {
    let bad_op = ShelfRule {
        field: RuleField::Author,
        op: RuleOp::Gte,
        value: "5".into(),
    };
    assert!(bad_op.validate().is_err());

    let empty = ShelfRule {
        field: RuleField::Tag,
        op: RuleOp::Is,
        value: "  ".into(),
    };
    assert!(empty.validate().is_err());

    let ok = ShelfRule {
        field: RuleField::Tag,
        op: RuleOp::Is,
        value: "Fantasy".into(),
    };
    assert!(ok.validate().is_ok());
}

#[test]
fn rule_validate_rejects_overlong_value_and_accepts_value_at_cap() {
    let overlong = ShelfRule {
        field: RuleField::Tag,
        op: RuleOp::Is,
        value: "a".repeat(SHELF_RULE_VALUE_MAX_LEN + 1),
    };
    assert!(overlong.validate().is_err());

    // Multibyte char: chars().count() == cap but len() (bytes) is double
    // that, so a regression to a byte-length check would fail this.
    let at_cap = ShelfRule {
        field: RuleField::Tag,
        op: RuleOp::Is,
        value: "é".repeat(SHELF_RULE_VALUE_MAX_LEN),
    };
    assert!(at_cap.validate().is_ok());
}

#[test]
fn create_validate_rejects_overlong_description_and_accepts_description_at_cap() {
    let base = CreateShelfRequest {
        kind: ShelfKind::Manual,
        name: "Best of 2026".into(),
        description: None,
        visibility: Visibility::Private,
        match_mode: None,
        rules: vec![],
        book_uuids: vec![],
    };

    let overlong = CreateShelfRequest {
        description: Some("a".repeat(SHELF_DESCRIPTION_MAX_LEN + 1)),
        ..base.clone()
    };
    assert!(overlong.validate().is_err());

    // Multibyte char: chars().count() == cap but len() (bytes) is double
    // that, so a regression to a byte-length check would fail this.
    let at_cap = CreateShelfRequest {
        description: Some("é".repeat(SHELF_DESCRIPTION_MAX_LEN)),
        ..base.clone()
    };
    assert!(at_cap.validate().is_ok());
}

#[test]
fn update_validate_rejects_overlong_description() {
    let overlong = UpdateShelfRequest {
        description: Some("a".repeat(SHELF_DESCRIPTION_MAX_LEN + 1)),
        ..Default::default()
    };
    assert!(overlong.validate().is_err());

    // Multibyte char: chars().count() == cap but len() (bytes) is double
    // that, so a regression to a byte-length check would fail this.
    let at_cap = UpdateShelfRequest {
        description: Some("é".repeat(SHELF_DESCRIPTION_MAX_LEN)),
        ..Default::default()
    };
    assert!(at_cap.validate().is_ok());
}

#[test]
fn create_validate_enforces_smart_invariants() {
    let smart = CreateShelfRequest {
        kind: ShelfKind::Smart,
        name: "Fantasy".into(),
        description: None,
        visibility: Visibility::Public,
        match_mode: Some(MatchMode::Any),
        rules: vec![ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        }],
        book_uuids: vec![],
    };
    assert!(smart.validate().is_ok());

    let no_rules = CreateShelfRequest {
        rules: vec![],
        ..smart.clone()
    };
    assert!(no_rules.validate().is_err());

    let smart_with_books = CreateShelfRequest {
        book_uuids: vec!["u".into()],
        ..smart.clone()
    };
    assert!(smart_with_books.validate().is_err());
}

#[test]
fn create_validate_enforces_manual_invariants() {
    let manual = CreateShelfRequest {
        kind: ShelfKind::Manual,
        name: "Best of 2026".into(),
        description: None,
        visibility: Visibility::Private,
        match_mode: None,
        rules: vec![],
        book_uuids: vec!["u1".into(), "u2".into()],
    };
    assert!(manual.validate().is_ok());

    let manual_with_rules = CreateShelfRequest {
        match_mode: Some(MatchMode::All),
        ..manual.clone()
    };
    assert!(manual_with_rules.validate().is_err());
}

#[test]
fn create_validate_rejects_blank_and_overlong_names() {
    let base = CreateShelfRequest {
        kind: ShelfKind::Manual,
        name: "  ".into(),
        description: None,
        visibility: Visibility::Private,
        match_mode: None,
        rules: vec![],
        book_uuids: vec![],
    };
    assert!(base.validate().is_err());

    let long = CreateShelfRequest {
        name: "x".repeat(SHELF_NAME_MAX_LEN + 1),
        ..base.clone()
    };
    assert!(long.validate().is_err());
}
