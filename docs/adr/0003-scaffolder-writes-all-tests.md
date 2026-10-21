# The Scaffolder writes all tests; Writers never touch them

Tests are written in full by the Scaffolder during scaffolding and reviewed at the scaffold Checkpoint. They are the executable spec Writers implement against. Sonnet-class output costs about 20× Haiku-class output, but tests cost only cents per Task, while one wrong test can trigger a whole chain of Writer retries and Revisions. Letting Writers write tests would also let the model being graded decide what "passing" means.

## Consequences

Because other Regions are still Placeholders while a Work Unit is verified, each Work Unit runs only its own filtered tests. The full Unit Check runs when the Task completes.
