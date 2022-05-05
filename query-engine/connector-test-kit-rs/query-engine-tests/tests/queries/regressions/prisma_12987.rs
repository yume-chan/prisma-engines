use query_engine_tests::*;

// Related issue: https://github.com/prisma/prisma/issues/11731
#[test_suite]
mod boolean_reduction {
    use indoc::indoc;
    use query_engine_tests::run_query;

    fn schema() -> String {
        let schema = indoc! {
            r#"
            model Test {
                #id(id, Int, @id)
                number Int
                string String
            }"#
        };

        schema.to_owned()
    }

    #[connector_test(schema(schema))]
    async fn boolean_reduction_should_work(runner: Runner) -> TestResult<()> {
        run_query!(
            &runner,
            r#"mutation { createOneTest(data: { id: 1, number: 10, string: "foo" }) { id } }"#
        );

        run_query!(
            &runner,
            r#"mutation { createOneTest(data: { id: 2, number: -10, string: "bar" }) { id } }"#
        );

        run_query!(
            &runner,
            r#"mutation { createOneTest(data: { id: 3, number: 5, string: "baz" }) { id } }"#
        );

        // A: number: { gt: 0 }
        // B: { string: "baz" },
        // C: { string: "nope" }
        // => `A * (B + C)`
        insta::assert_snapshot!(
            run_query!(
                &runner,
                r#"
              {
                findManyTest(where: {
                  number: { gt: 0 }
                  OR: [
                    { string: "baz" },
                    { string: "nope" }
                  ]
                }) {
                  id
                }
              }
              "#
            ),
            @r###"{"data":{"findManyTest":[{"id":3}]}}"###
        );

        // => `A * (B + C)` simplified is (A * B) + (A * C)
        insta::assert_snapshot!(
            run_query!(
                &runner,
                r#"
            {
              findManyTest(where: {
                OR: [
                  { AND: [{ number: { gt: 0 } }, { string: "baz" }] },
                  { AND: [{ number: { gt: 0 } }, { string: "nope" }] },
                ]
              }) {
                id
              }
            }
            "#
            ),
            @r###"{"data":{"findManyTest":[{"id":3}]}}"###
        );

        Ok(())
    }
}
