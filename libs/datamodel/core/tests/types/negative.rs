use crate::common::*;

#[test]
fn should_fail_on_native_type_with_invalid_datasource_name() {
    let dml = r#"
        datasource db {
          provider = "postgres"
          url = "postgresql://"
        }

        model Blog {
            id     Int    @id
            bigInt Int    @pg.Integer
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}

#[test]
fn should_fail_on_native_type_with_invalid_number_of_arguments() {
    let dml = r#"
        datasource pg {
          provider = "postgres"
          url = "postgresql://"
        }

        model Blog {
            id     Int    @id
            bigInt Int    @pg.Integer
            foobar String @pg.VarChar(2, 3, 4)
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}

#[test]
fn should_fail_on_native_type_with_unknown_type() {
    let dml = r#"
        datasource pg {
          provider = "postgres"
          url = "postgresql://"
        }

        model Blog {
            id     Int    @id
            bigInt Int    @pg.Numerical(3, 4)
            foobar String @pg.VarChar(5)
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}

#[test]
fn should_fail_on_native_type_with_incompatible_type() {
    let dml = r#"
        datasource pg {
          provider = "postgres"
          url = "postgresql://"
        }

        model Blog {
            id     Int    @id
            foobar Boolean @pg.VarChar(5)
            foo Int @pg.BigInt
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}

#[test]
fn should_fail_on_native_type_with_invalid_arguments() {
    let dml = r#"
        datasource pg {
          provider = "postgres"
          url = "postgresql://"
        }

        model Blog {
            id     Int    @id
            foobar String @pg.VarChar(a)
        }
    "#;

    let expected = expect![[r#"
        [1;91merror[0m: [1mExpected a numeric value, but failed while parsing "a": invalid digit found in string.[0m
          [1;94m-->[0m  [4mschema.prisma:9[0m
        [1;94m   | [0m
        [1;94m 8 | [0m            id     Int    @id
        [1;94m 9 | [0m            foobar String @[1;91mpg.VarChar(a)[0m
        [1;94m   | [0m
    "#]];

    expect_error(dml, &expected)
}

#[test]
fn should_fail_on_native_type_in_unsupported_postgres() {
    let dml = r#"
        datasource pg {
          provider = "postgres"
          url = "postgresql://"
        }

        model Blog {
            id              Int    @id
            decimal         Unsupported("Decimal(10,2)")
            text            Unsupported("Text")
            unsupported     Unsupported("Some random stuff")
            unsupportes2    Unsupported("Some random (2,5) do something")
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}

#[test]
fn should_fail_on_native_type_in_unsupported_mysql() {
    let dml = r#"
        datasource pg {
          provider = "mysql"
          url = "mysql://"
        }

        model Blog {
            id          Int    @id
            text        Unsupported("Text")
            decimal     Unsupported("Float")
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}

#[test]
fn should_fail_on_native_type_in_unsupported_sqlserver() {
    let dml = r#"
        datasource pg {
          provider = "sqlserver"
          url = "sqlserver://"
        }

        model Blog {
            id          Int    @id
            text        Unsupported("Text")
            decimal     Unsupported("Real")
            TEXT        Unsupported("TEXT")
        }
    "#;

    let expected = expect![[""]];

    expected.assert_eq(&parse_error(dml));
}
