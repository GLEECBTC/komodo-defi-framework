# PR #2714 Review Replies Draft

## Comment 1 — AccountResourceMessage docs (api.rs:576)
ID: 2888485932

> i really just meant with this question that we should have some sort of intro to what `AccountResourceMessage`. it is mentioned in there without any formal definition and the reader is expected to know what it is. ctrl+f across the code base doesn't show any other occurrences of it.

Reply: TODO

## Comment 2 — Eliminate intermediate struct (api.rs:595)
ID: 2888491661

> yeah i think that's better. and that was why i was asking that question.

Reply: TODO

## Comment 3 — Validate timestamp in validated_header() (api.rs:739)
ID: 2888550218

> why would `current_block()` be ok with a block that has a negative timestamp? i say we better validate the timestamp for both cases (i.e. validate the timestamp in `validate_header()`)

Reply: TODO

## Comment 4 — Check max balance in test (tron_tests.rs:913)
ID: 2888876607

> we should rather do something with this balance. like check that the "max" that was sent is equal to this balance (- fee miscalc clearance)

Reply: TODO

## Comment 5 — Why isn't this test cross? (withdraw.rs:229)
ID: 2888964145

> why isn't this test cross?

Reply: TODO
