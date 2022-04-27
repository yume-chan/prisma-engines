use query_engine_tests::*;

#[test_suite(only(MongoDb))]
mod mongodb {
    use indoc::indoc;

    fn a() -> String {
        let schema = indoc! {
            r#"
            model Test {
                id   String @id @map("_id")
                list String[]
            }
            "#
        };
        schema.to_owned()
    }

    fn b() -> String {
        let schema = indoc! {
            r#"
            model Test {
                id   String @id @map("_id") @test.ObjectId
                list String[] @test.ObjectId
            }
            "#
        };
        schema.to_owned()
    }

    #[schema_drift_test(schema_a(a), schema_b(b))]
    async fn coerces_id_strings_to_oid(runner_a: Runner, runner_b: Runner) -> TestResult<()> {
        runner_a
            .query(r#"mutation { createOneTest(data: { id: "6267b40792e1024445cde5ea" }) { id } }"#)
            .await?
            .assert_success();

        assert_query!(
            runner_b,
            r#"query { findUniqueTest(where: { id: "6267b40792e1024445cde5ea" }) { id } }"#,
            r#"{"data":{"findUniqueTest":{"id":"6267b40792e1024445cde5ea"}}}"#
        );

        // where ID in list

        assert_query!(
            runner_b,
            r#"query { findManyTest(where: { id: { in: ["6269240892e1024445cde5eb", "6267b40792e1024445cde5ea"] }}) { id } }"#,
            r#"{"data":{"findManyTest":[{"id":"6267b40792e1024445cde5ea"}]}}"#
        );

        Ok(())
    }

    #[schema_drift_test(schema_a(a), schema_b(b))]
    async fn coerces_string_array_to_oid_array(runner_a: Runner, runner_b: Runner) -> TestResult<()> {
        runner_a
            .query(r#"mutation { createOneTest(data: { id: "6267b40792e1024445cde5ea", list: ["6269240892e1024445cde5eb", "6269241e92e1024445cde5ec"] }) { id } }"#)
            .await?
            .assert_success();

        // where oid in list

        assert_query!(
            runner_b,
            r#"query { findManyTest(where: { list: { has: "6269240892e1024445cde5eb" } }) { id } }"#,
            r#"{"data":{"findManyTest":[{"id":"6267b40792e1024445cde5ea"}]}}"#
        );

        Ok(())
    }
}
