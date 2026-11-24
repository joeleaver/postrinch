//! sam@lindenworks.example's mail: Linden Works, a small design studio,
//! its two clients, its tools and its lists. Everyone is made up
//! (people.rs).

use crate::DemoResult;
use crate::accounts::{
    CLIENTS, FERNHILL, FORUMS, HARBOR, HIRING, IMPORTANT, INBOX, INVOICES, PRIMARY, PROMOTIONS,
    SOCIAL, STUDIO, TO_REVIEW, UPDATES, WAITING,
};
use crate::calendar::{WORKSHOP_PLACE, WORKSHOP_TITLE, workshop, workshop_invitation};
use crate::clock::{Clock, ago, day};
use crate::newsletters;
use crate::pdf;
use crate::people::*;
use crate::threads::{Attachment, ThreadSpec, msg, thread};

const ME: Person = SAM_WORK;

pub fn threads(clock: &Clock) -> DemoResult<Vec<ThreadSpec>> {
    let mut all = vec![harbor_round_two(), invitation(clock)?];
    all.extend(clients());
    all.extend(studio());
    all.extend(tools_and_lists());
    Ok(all)
}

/// The project thread the main screenshot opens.
fn harbor_round_two() -> ThreadSpec {
    let notes = pdf::document(&[
        "Harbor Coffee - rebrand, round 2 notes",
        "",
        "1. Wordmark: keep the lighter weight; tighten the H-a spacing.",
        "2. Colour: the harbour blue reads cold on kraft. Try it warmer.",
        "3. Cup sleeve: love the wave pattern. Less of it on the small size.",
        "4. Bags: add the roast date panel back, staff rely on it.",
        "",
        "Priya, Ollie",
    ]);
    thread(
        ME,
        "Harbor Coffee rebrand: round 2 feedback",
        &[INBOX, PRIMARY, IMPORTANT, HARBOR],
    )
    .starred()
    .msg(
        msg(
            ME,
            &[PRIYA],
            day(6, 10, 20),
            "Hi Priya,\n\nRound 2 is up on the review board: the wordmark in two weights, \
             the warmer palette, and the cup sleeve and bag in context.\n\nCould you and \
             Ollie send notes by Friday? Then we'll have finals ready for the packaging \
             workshop.\n\nBest,\nSam",
        )
        .cc(&[THEO]),
    )
    .msg(
        msg(
            PRIYA,
            &[ME],
            day(5, 16, 45),
            "Thanks Sam, these are great. Ollie and I are going through them tomorrow \
             morning with the shop leads.\n\nFirst reaction: the lighter wordmark, \
             definitely.\n\nPriya",
        )
        .cc(&[THEO, OLLIE]),
    )
    .msg(
        msg(
            THEO,
            &[PRIYA, ME],
            day(5, 17, 2),
            "Great to hear. If it helps, I can mock the lighter one up on the shop sign as \
             well, so you can see it at a distance.\n\nTheo",
        )
        .cc(&[OLLIE]),
    )
    .msg(
        msg(
            PRIYA,
            &[ME],
            day(3, 9, 30),
            "Here are our notes, attached. Short version: lighter wordmark, warmer blue, \
             less wave on the small sleeve, and the roast date panel has to come back on \
             the bags (the staff were very clear about that one!).\n\nPriya",
        )
        .cc(&[THEO, OLLIE])
        .attach(Attachment::new(
            "round-2-notes.pdf",
            "application/pdf",
            &notes,
        )),
    )
    .msg(
        msg(
            ME,
            &[PRIYA],
            day(2, 11, 10),
            "All clear, thank you both. We'll have the revised set to you before the \
             workshop. Theo is doing the sign mock-up this week.\n\nSam",
        )
        .cc(&[THEO, OLLIE]),
    )
    .msg(
        msg(
            PRIYA,
            &[ME],
            ago(24),
            "Perfect. One more thing: could we see the sleeve in the darker kraft too? The \
             supplier just told us the light one is out until spring.\n\nPriya",
        )
        .cc(&[THEO, OLLIE])
        .unread(),
    )
}

/// An invitation with its calendar part, not answered yet.
fn invitation(clock: &Clock) -> DemoResult<ThreadSpec> {
    let (start, end) = workshop(clock)?;
    let when = format!(
        "{} {} – {}",
        start.strftime("%a %b %-d,"),
        start.strftime("%-I:%M%P"),
        end.strftime("%-I:%M%P")
    );
    let subject = format!("Invitation: {WORKSHOP_TITLE} @ {when}");
    let text = format!(
        "Priya Nand has invited you to an event.\n\n{WORKSHOP_TITLE}\nWhen: {when}\n\
         Where: {WORKSHOP_PLACE}\nWho: Priya Nand (organizer), Ollie Grant, Sam Calloway, \
         Theo Marsh\n\nBring the cup sleeve mock-ups and the two box options."
    );
    Ok(thread(ME, &subject, &[INBOX, PRIMARY, HARBOR]).msg(
        msg(PRIYA, &[ME], ago(95), &text)
            .cc(&[THEO, OLLIE])
            .invite(workshop_invitation(clock)?)
            .unread(),
    ))
}

fn clients() -> Vec<ThreadSpec> {
    let quote = pdf::document(&[
        "Ridgeline Print - Quote Q-2291",
        "For: Linden Works, re Harbor Coffee",
        "",
        "Cup sleeves, kraft, 2 colours, 10,000     $1,240.00",
        "Coffee bags, 250 g, matte, 5,000          $2,150.00",
        "Proofs and setup                            $180.00",
        "",
        "Total before tax                          $3,570.00",
        "Valid for 30 days.",
    ]);
    vec![
        thread(
            ME,
            "Menu boards: can we move the call?",
            &[INBOX, PRIMARY, FERNHILL],
        )
        .msg(
            msg(
                MARCUS,
                &[ME],
                ago(70),
                "Hi Sam,\n\nSomething came up at the bakery on Tuesday morning. Could we do \
                 the call at 10:30 instead of 10? Sorry for the shuffle.\n\nMarcus",
            )
            .unread(),
        ),
        thread(
            ME,
            "Quote for Harbor Coffee cups and bags",
            &[INBOX, PRIMARY, HARBOR, WAITING],
        )
        .msg(
            msg(
                RIDGELINE,
                &[ME],
                day(2, 15, 5),
                "Hi Sam,\n\nAs discussed, our quote for the sleeves and bags is attached. \
                 Proofs take three working days once we have final files.\n\nRidgeline Print",
            )
            .attach(Attachment::new(
                "quote-Q-2291.pdf",
                "application/pdf",
                &quote,
            )),
        )
        .msg(msg(
            ME,
            &[RIDGELINE],
            day(2, 16, 30),
            "Thanks! I'll share it with the client and come back to you next week.",
        )),
        thread(ME, "Proposal: Fernhill Bakery spring menu", &[FERNHILL]).msg(msg(
            ME,
            &[MARCUS],
            day(4, 14, 0),
            "Hi Marcus,\n\nAs promised, here's what we'd propose for the spring menu \
                 boards: three boards, the chalk style we tried, and a printed card for \
                 the counter. Two weeks from your go-ahead.\n\nSam",
        )),
        thread(ME, "Signed proposal attached", &[FERNHILL, CLIENTS]).msg(msg(
            MARCUS,
            &[ME],
            day(15, 11, 40),
            "Signed and attached. Looking forward to it!\n\nMarcus",
        )),
        thread(ME, "Kickoff notes", &[HARBOR, CLIENTS]).msg(
            msg(
                ME,
                &[PRIYA, OLLIE],
                day(20, 17, 15),
                "Thanks for a great kickoff. My notes: keep the anchor, lose the script, \
                 warmer than the competition, and the shop staff are the audience as much \
                 as the customers.\n\nSam",
            )
            .cc(&[THEO]),
        ),
    ]
}

fn studio() -> Vec<ThreadSpec> {
    let timeline = b"week,task,owner\n1,Round 2 revisions,Sam\n1,Shop sign mock-up,Theo\n2,Packaging workshop,All\n2,Final files to Ridgeline,Sam\n3,Proofs,Ridgeline\n";
    vec![
        thread(ME, "Moodboard v3 is up", &[INBOX, PRIMARY, STUDIO, TO_REVIEW])
            .msg(msg(THEO, &[ME], day(1, 16, 50),
                "Pushed v3 of the Fernhill moodboard: more chalk, less script, and a couple \
                 of hand-painted sign references from the old market hall.\n\nThe timeline \
                 for Harbor is attached too, tell me if the dates look wrong.\n\nTheo")
                .attach(Attachment::new("harbor-timeline.csv", "text/csv", timeline))
                .unread()),
        thread(ME, "Lunch Wednesday?", &[INBOX, PRIMARY])
            .msg(msg(THEO, &[ME], day(1, 11, 5), "Juniper Café, noon? I owe you one."))
            .msg(msg(ME, &[THEO], day(1, 11, 9), "You do. See you there.")),
        thread(ME, "Q1 planning: draft agenda", &[INBOX, PRIMARY, STUDIO, WAITING])
            .msg(msg(INES, &[ME, THEO], day(2, 9, 0),
                "Draft agenda for the planning morning, before the offsite:\n\n1. How the \
                 year went, briefly\n2. Clients for Q1, and who leads which\n3. Hiring: the \
                 junior role\n4. Tools: do we keep Tracklight?\n\nAdd anything by Thursday.\n\nInês"))
            .msg(msg(ME, &[INES, THEO], day(2, 9, 45),
                "Could we add pricing? The bakery work took twice what we quoted.")),
        thread(ME, "Re: Offsite on Friday", &[INBOX, PRIMARY, STUDIO])
            .msg(msg(INES, &[ME, THEO], day(3, 14, 20),
                "Cedar Lodge is booked for Friday. Train at 8:40; I'll bring breakfast."))
            .msg(msg(THEO, &[INES, ME], day(3, 14, 41), "I'll bring the good coffee.")),
        thread(ME, "Application: Junior designer (Ana Kovač)", &[INBOX, PRIMARY, HIRING])
            .msg(msg(CAREERS, &[ME], day(3, 10, 2),
                "A new application for Junior designer.\n\nName: Ana Kovač\nPortfolio: \
                 anakovac.example\n\n\"I've followed the studio's work since the library \
                 wayfinding project and would love to learn how you run a brief from first \
                 sketch to print.\"")),
        thread(ME, "Application: Junior designer (Tomás Reyes)", &[HIRING])
            .msg(msg(CAREERS, &[ME], day(7, 15, 12),
                "A new application for Junior designer.\n\nName: Tomás Reyes\nPortfolio: \
                 tomasreyes.example")),
        thread(ME, "Your storage is almost full", &[INBOX, UPDATES])
            .msg(msg(STUDIO_IT, &[ME], day(4, 8, 0),
                "The studio drive is 92% full. Please archive finished projects to the \
                 archive drive this week.")),
    ]
}

fn tools_and_lists() -> Vec<ThreadSpec> {
    vec![
        thread(
            ME,
            "Invoice LW-0142 has been paid",
            &[INBOX, UPDATES, INVOICES],
        )
        .msg(msg(
            LEDGERLY,
            &[ME],
            day(1, 13, 0),
            "Fernhill Bakery paid invoice LW-0142 ($2,400.00). The money should reach \
                 your account in two working days.",
        )),
        thread(
            ME,
            "Reminder: invoice LW-0141 is due in 3 days",
            &[INBOX, UPDATES, INVOICES],
        )
        .msg(
            msg(
                LEDGERLY,
                &[ME],
                day(2, 7, 0),
                "Invoice LW-0141 to Harbor Coffee ($4,850.00) is due in 3 days.",
            )
            .unread(),
        ),
        thread(ME, "Invoice LW-0140 has been paid", &[INVOICES]).msg(msg(
            LEDGERLY,
            &[ME],
            day(16, 13, 0),
            "Harbor Coffee paid invoice LW-0140 ($3,100.00).",
        )),
        thread(
            ME,
            "[Harbor Coffee] Theo moved \"Cup sleeve\" to Review",
            &[INBOX, UPDATES],
        )
        .msg(
            msg(
                TRACKLIGHT,
                &[ME],
                day(1, 17, 3),
                "Theo Marsh moved \"Cup sleeve\" from Doing to Review.",
            )
            .unread(),
        ),
        thread(
            ME,
            "[Harbor Coffee] 3 tasks due this week",
            &[INBOX, UPDATES],
        )
        .msg(msg(
            TRACKLIGHT,
            &[ME],
            day(3, 7, 0),
            "Round 2 revisions (Sam), Shop sign mock-up (Theo), Final files (Sam).",
        )),
        thread(ME, "New this month: Juniper Serif", &[INBOX, PROMOTIONS]).msg(
            msg(FIGMENT, &[ME], day(2, 15, 0), newsletters::FIGMENT_TEXT)
                .html(newsletters::FIGMENT_HTML),
        ),
        thread(
            ME,
            "Type Talk digest: kerning tables, variable fonts",
            &[INBOX, FORUMS],
        )
        .msg(msg(
            TYPE_TALK,
            &[ME],
            day(1, 5, 30),
            "Is optical kerning ever better than the font's own? (9 replies)\nVariable \
                 fonts in print workflows (5 replies)",
        )),
        thread(
            ME,
            "3 people liked your project \"Library wayfinding\"",
            &[INBOX, SOCIAL],
        )
        .msg(msg(
            FOLIO,
            &[ME],
            day(4, 19, 0),
            "Your project is getting noticed. See who liked it on Folio.",
        )),
    ]
}
