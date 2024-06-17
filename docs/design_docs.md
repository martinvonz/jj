# Jujutsu Design Docs

Jujutsu uses Design Docs to drive technical decisions on large projects and it
is the place to discuss your proposed design or new component. It is a very 
thorough process, in which the design doc must be approved before PRs for the
feature will be accepted. It shares some similarities with [Rust RFCs] but 
mostly addresses _technical_ problems and  gauges the technical and social 
concerns of all stakeholders.

So if you want to start building the native backend or the server component for
Jujutsu, you'll need to go through this process. 

## Process

1. Add a new markdown document to `docs/design`, named after your improvement 
   or project. 
1. Describe the current state of the world and the things you want to improve.
1. Wait for the Maintainers and Stakeholders to show up. 
1. Iterate until everyone accepts the change in normal codereview fashion.
   

[Rust RFCs]: https://github.com/rust-lang/rfcs 

## Blueprint (Template)

You can find the base template of a new Design Doc 
[here](design_doc_blueprint.md).

