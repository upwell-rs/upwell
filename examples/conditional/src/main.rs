//! Runs the conditional-components scenario and prints each step: switch to the custom
//! provider, attempt a switch back whose replacement fails and rolls back, then switch back.

use upwell::AppError;
use upwell_example_conditional::{AuthMode, Demo, DemoError};

#[tokio::main]
async fn main() -> Result<(), DemoError> {
    let demo = Demo::start(AuthMode::Default, 8).await?;

    println!("started: {}", demo.banner().text());
    demo.ledger().open();
    demo.ledger().open();
    print_state(&demo);

    demo.switch(AuthMode::Custom, 8).await?;
    println!("\nswitched to custom: {}", demo.banner().text());
    demo.ledger().open();
    print_state(&demo);

    match demo.switch(AuthMode::Default, 1).await {
        Err(DemoError::App(AppError::TransitionRejected(rejection))) => {
            println!(
                "\nswitch back with max_sessions = 1 rolled back: {:?}",
                rejection.report().outcome()
            );
        }

        other => panic!("the session limit must reject the switch: {other:?}"),
    }

    println!("still: {}", demo.banner().text());
    print_state(&demo);

    demo.switch(AuthMode::Default, 8).await?;
    println!("\nswitched back to default: {}", demo.banner().text());
    print_state(&demo);

    println!("\naudit:");

    for event in demo.audit().take() {
        println!("  {event:?}");
    }

    Ok(())
}

fn print_state(demo: &Demo) {
    let gate = demo.gate();
    let ledger = demo.ledger();

    println!(
        "  login 'custom:alice' -> {:?}, login 'bob' -> {:?}",
        gate.login("custom:alice"),
        gate.login("bob")
    );
    println!(
        "  ledger ({}) holds {} of {} sessions",
        ledger.scheme(),
        ledger.sessions(),
        ledger.limit()
    );
}
