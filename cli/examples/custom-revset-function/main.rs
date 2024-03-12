use std::any::Any;
use std::rc::Rc;

use itertools::Itertools;
use jj_cli::cli_util::CliRunner;
use jj_lib::backend::CommitId;
use jj_lib::object_id::ObjectId;
use jj_lib::revset::{
    expect_one_argument, parse_expression_rule, ResolvedExpression, ResolvedExpressionExtension,
    Revset, RevsetEvaluationError, RevsetExpression, RevsetExpressionExtension, RevsetFunctionMap,
    TransformedExpressionResult,
};

// Returns only those commits in `inner` that tie for the most numbers in their
// ids.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MostNumbers {
    inner: Rc<RevsetExpression>,
}

impl RevsetExpressionExtension for MostNumbers {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(
        &self,
        transform_fn: &mut dyn FnMut(&Rc<RevsetExpression>) -> TransformedExpressionResult,
    ) -> TransformedExpressionResult {
        if let Some(transformed) = transform_fn(&self.inner)? {
            let most_numbers = MostNumbers { inner: transformed };
            Ok(Some(Rc::new(RevsetExpression::Extension(Rc::new(
                Box::new(most_numbers),
            )))))
        } else {
            Ok(None)
        }
    }

    fn resolve(
        &self,
        resolve_fn: &dyn Fn(&RevsetExpression) -> ResolvedExpression,
    ) -> ResolvedExpression {
        let inner = Box::new(resolve_fn(self.inner.as_ref()));
        ResolvedExpression::Extension(Rc::new(Box::new(MostNumbersResolved { inner })))
    }

    fn eq(&self, other: &dyn RevsetExpressionExtension) -> bool {
        other
            .as_any()
            .downcast_ref::<MostNumbers>()
            .map(|other| self == other)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MostNumbersResolved {
    inner: Box<ResolvedExpression>,
}

fn num_digits_in_id(id: &CommitId) -> usize {
    id.hex().chars().filter(|ch| ch.is_ascii_digit()).count()
}

impl ResolvedExpressionExtension for MostNumbersResolved {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn evaluate<'index>(
        &self,
        evaluate_fn: &dyn Fn(
            &ResolvedExpression,
        ) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError>,
    ) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError> {
        let inner = evaluate_fn(self.inner.as_ref())?;

        // A more advanced implementation would do smart things like stop at `num_digits
        // == max_possible`, and not store all the commits in memory. For brevity we
        // reuse a provided revset implementation.
        let num_digits = inner.iter().map(|id| num_digits_in_id(&id)).max();
        let commits = if let Some(max) = num_digits {
            inner
                .iter()
                .filter(|id| num_digits_in_id(id) == max)
                .collect_vec()
        } else {
            vec![]
        };

        evaluate_fn(&ResolvedExpression::Commits(commits))
    }

    fn eq(&self, other: &dyn ResolvedExpressionExtension) -> bool {
        other
            .as_any()
            .downcast_ref::<MostNumbersResolved>()
            .map(|other| self == other)
            .unwrap_or(false)
    }
}

fn revset_extension() -> RevsetFunctionMap {
    let mut map = RevsetFunctionMap::new();
    map.insert(
        "most_numbers",
        Box::new(|name, arguments_pair, state| {
            let arg = expect_one_argument(name, arguments_pair)?;
            let expression = parse_expression_rule(arg.into_inner(), state)?;
            let most_numbers = MostNumbers { inner: expression };
            Ok(Rc::new(RevsetExpression::Extension(Rc::new(Box::new(
                most_numbers,
            )))))
        }),
    );
    map
}

fn main() -> std::process::ExitCode {
    CliRunner::init()
        .set_revset_function_map_extension(revset_extension())
        .run()
}
