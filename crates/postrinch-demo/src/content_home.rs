//! sam@example.com's mail: family and friends, a landlord, shops,
//! newsletters, a bank, a trip. Everyone is made up (people.rs).

use crate::accounts::{
    BILLS, FOLLOW_UP, FORUMS, GARDEN, HOME, IMPORTANT, INBOX, LISBON, PRIMARY, PROMOTIONS,
    READ_LATER, RECEIPTS, SOCIAL, TRAVEL, UPDATES,
};
use crate::clock::{ago, day};
use crate::newsletters;
use crate::pdf;
use crate::people::*;
use crate::threads::{Attachment, ThreadSpec, msg, thread};

const ME: Person = SAM_HOME;

pub fn threads() -> Vec<ThreadSpec> {
    let mut all = family_and_friends();
    all.extend(shops_and_services());
    all.extend(lists());
    all
}

fn family_and_friends() -> Vec<ThreadSpec> {
    vec![
        thread(ME, "Dinner on Sunday?", &[INBOX, PRIMARY, IMPORTANT, HOME])
            .msg(msg(RUTH, &[ME], day(2, 18, 40),
                "Hi love,\n\nAre you free for dinner on Sunday? I'm making the lentil soup \
                 you like, and your aunt Bea might come by after.\n\nLet me know by Friday \
                 so I can plan the shopping.\n\nMom"))
            .msg(msg(ME, &[RUTH], day(2, 20, 5),
                "Yes please! I'll bring bread from the market. Can I bring anything else?\n\nSam"))
            .msg(msg(RUTH, &[ME], ago(38),
                "Lovely. Bring the bread and nothing else. Five thirty, and don't be late \
                 this time.\n\nMom").unread()),
        thread(ME, "Climbing Tuesday?", &[INBOX, PRIMARY])
            .msg(msg(JONAH, &[ME], day(1, 9, 12),
                "Boulder Barn has the new wall open. Tuesday at 6:30? I can drive."))
            .msg(msg(ME, &[JONAH], day(1, 9, 40),
                "In. I'll meet you there, I have a thing until six.")),
        thread(ME, "Photos from the lake", &[INBOX, PRIMARY, FOLLOW_UP])
            .msg(msg(MAYA, &[ME], day(4, 21, 3),
                "Finally sorted through them. The one of you falling off the paddleboard is \
                 my favourite, obviously.\n\nThe album is on my drive; tell me which ones you \
                 want printed and I'll do an order this week.\n\nMaya").unread()),
        thread(ME, "Book club pick", &[INBOX, PRIMARY])
            .msg(msg(MAYA, &[ME, JONAH], day(6, 19, 15),
                "My turn to choose. Shortlist: something short and Scandinavian, something \
                 long and Russian, or the bird book everyone keeps talking about. Votes?"))
            .msg(msg(JONAH, &[MAYA, ME], day(6, 19, 47), "Birds. Always birds."))
            .msg(msg(ME, &[MAYA, JONAH], day(5, 8, 30),
                "Short and Scandinavian, I have a busy month. But I'll read about birds if outvoted."))
            .msg(msg(MAYA, &[ME, JONAH], day(5, 12, 2),
                "Birds it is. Thursday, my place, seven. I'll make the dip.")),
        thread(ME, "Radiator in the back bedroom", &[INBOX, PRIMARY, HOME])
            .msg(msg(ME, &[DANA], day(3, 8, 2),
                "Hi Dana,\n\nThe radiator in the back bedroom has stopped warming up, even \
                 after bleeding it. Could someone take a look this week? I'm home most \
                 mornings.\n\nThanks,\nSam Calloway, flat 3"))
            .msg(msg(DANA, &[ME], ago(185),
                "Hi Sam,\n\nSorry about that. Leo from Ashby Heating can come Thursday \
                 between 9 and 11. Does that work? He'll call when he's on his way.\n\nDana \
                 Whitfield\nWhitfield Homes").unread()),
        thread(ME, "Recipe you asked for", &[HOME])
            .msg(msg(RUTH, &[ME], day(9, 16, 20),
                "Here it is, as promised: the plum cake.\n\n250 g plums, halved\n125 g \
                 butter\n125 g sugar\n2 eggs\n150 g flour\n1 tsp baking powder\nA pinch of \
                 cinnamon\n\nCream the butter and sugar, beat in the eggs, fold in the flour. \
                 Plums on top, cut side up. 50 minutes at 180.\n\nMom")),
    ]
}

fn shops_and_services() -> Vec<ThreadSpec> {
    let receipt = pdf::document(&[
        "Paper Crane Shop",
        "Receipt PC-20418",
        "",
        "1 x A5 dot-grid notebook, sage         $18.00",
        "1 x Brass fountain pen, fine nib        $46.00",
        "2 x Ink cartridges, blue-black            $9.00",
        "Shipping                                   $5.00",
        "",
        "Total                                     $78.00",
        "Paid by card ending 4242",
    ]);
    let ticket = pdf::document(&[
        "Northline Rail - e-ticket",
        "Booking NR-7Q4K2",
        "Riverside Central to Harbor Point",
        "Saturday, departs 08:15, coach C, seat 42",
        "Show this ticket on your phone or printed.",
    ]);
    vec![
        thread(
            ME,
            "Your receipt from Paper Crane Shop (PC-20418)",
            &[INBOX, PRIMARY, RECEIPTS],
        )
        .msg(
            msg(
                PAPER_CRANE,
                &[ME],
                day(2, 11, 4),
                "Thank you for your order!\n\nReceipt PC-20418\nA5 dot-grid notebook, sage — \
                 $18.00\nBrass fountain pen, fine nib — $46.00\nInk cartridges, blue-black \
                 (2) — $9.00\nShipping — $5.00\nTotal — $78.00\n\nYour receipt is attached \
                 as a PDF. We'll write again when your parcel ships.\n\nPaper Crane Shop",
            )
            .attach(Attachment::new(
                "receipt-PC-20418.pdf",
                "application/pdf",
                &receipt,
            )),
        ),
        thread(
            ME,
            "Your Paper Crane order has shipped",
            &[INBOX, UPDATES, RECEIPTS],
        )
        .msg(
            msg(
                PAPER_CRANE,
                &[ME],
                day(1, 15, 30),
                "Good news: order PC-20418 is on its way. It should arrive in two to three \
                 working days.\n\nTracking number: 1Z-DEMO-55021",
            )
            .unread(),
        ),
        thread(
            ME,
            "Your e-ticket: Riverside Central → Harbor Point",
            &[INBOX, PRIMARY, TRAVEL],
        )
        .starred()
        .msg(
            msg(
                RAIL,
                &[ME],
                day(3, 13, 12),
                "Your booking is confirmed.\n\nBooking NR-7Q4K2\nSaturday, 08:15 from \
                 Riverside Central, coach C, seat 42.\n\nYour e-ticket is attached.",
            )
            .attach(Attachment::new(
                "e-ticket-NR-7Q4K2.pdf",
                "application/pdf",
                &ticket,
            )),
        ),
        thread(
            ME,
            "Booking confirmed: Rua das Flores apartment",
            &[INBOX, PRIMARY, TRAVEL, LISBON],
        )
        .msg(msg(
            CASA_AZUL,
            &[ME],
            day(6, 10, 45),
            "Olá Sam,\n\nYour stay is confirmed: the Rua das Flores apartment, four \
                 nights in April, for two guests.\n\nCheck-in is from 15:00; we'll send the \
                 door code the day before you arrive.\n\nAté breve,\nCasa Azul Stays",
        )),
        thread(
            ME,
            "Appointment reminder: Wednesday, 8:00",
            &[INBOX, PRIMARY],
        )
        .msg(msg(
            DENTIST,
            &[ME],
            day(1, 7, 0),
            "This is a reminder of your check-up with Dr. Okoro on Wednesday at 8:00. \
                 Reply C to confirm or call us to move it.\n\nBrightside Dental",
        )),
        thread(ME, "Your statement is ready", &[INBOX, UPDATES, BILLS]).msg(msg(
            BANK,
            &[ME],
            day(4, 6, 30),
            "Your monthly statement for the account ending 0917 is ready to view in \
                 online banking.\n\nCobalt Bank never asks for your password by email.",
        )),
        thread(ME, "Your water bill", &[BILLS]).msg(msg(
            WATER,
            &[ME],
            day(12, 9, 0),
            "Your bill for the last quarter is $64.20, due in 21 days. Pay online or \
                 set up automatic payments in your account.",
        )),
        thread(ME, "Your bulbs are on their way", &[GARDEN, RECEIPTS]).msg(msg(
            NURSERY,
            &[ME],
            day(10, 14, 22),
            "We've packed your order: 30 mixed tulip bulbs and 20 crocus. Plant them \
                 before the ground freezes, pointy end up!\n\nHollow Oak Nursery",
        )),
        thread(
            ME,
            "Receipt for your order at Hollow Oak Nursery",
            &[RECEIPTS],
        )
        .msg(msg(
            NURSERY,
            &[ME],
            day(13, 11, 2),
            "Thanks for your order of $31.50. Your receipt number is HO-3381.",
        )),
        thread(ME, "Your hold is ready to collect", &[INBOX, UPDATES]).msg(msg(
            LIBRARY,
            &[ME],
            day(2, 8, 15),
            "A Field Guide to Clouds is waiting for you at the Riverside branch. We'll \
                 keep it for seven days.",
        )),
    ]
}

fn lists() -> Vec<ThreadSpec> {
    vec![
        thread(
            ME,
            "Field Notes #112: The quiet art of maintenance",
            &[INBOX, UPDATES, READ_LATER],
        )
        .msg(
            msg(
                FIELD_NOTES,
                &[ME],
                day(1, 6, 0),
                newsletters::FIELD_NOTES_TEXT,
            )
            .html(newsletters::FIELD_NOTES_HTML)
            .unread(),
        ),
        thread(
            ME,
            "Field Notes #111: Paths worn by walking",
            &[UPDATES, READ_LATER],
        )
        .msg(msg(
            FIELD_NOTES,
            &[ME],
            day(8, 6, 0),
            "This week: desire paths, the ones people make by walking where the \
                 sidewalk isn't, and what planners learn from them.",
        )),
        thread(
            ME,
            "Street tree planting, Saturday morning",
            &[INBOX, PRIMARY],
        )
        .msg(msg(
            NEIGHBORS,
            &[ME],
            day(3, 17, 30),
            "We're planting twelve new street trees on Elm Row this Saturday from 9. \
                 Gloves and coffee provided; bring a spade if you have one.\n\nElm Row \
                 Neighbors",
        )),
        thread(ME, "Jonah Pike shared a route with you", &[INBOX, SOCIAL]).msg(
            msg(
                TRAILMATES,
                &[ME],
                day(2, 12, 40),
                "Jonah Pike shared \"Ridge loop, 14 km\" with you. Open Trailmates to see \
                 the route.",
            )
            .unread(),
        ),
        thread(ME, "Maya Ortiz commented on your photo", &[INBOX, SOCIAL]).msg(msg(
            TRAILMATES,
            &[ME],
            day(5, 20, 1),
            "Maya Ortiz: \"Best view of the trip.\"",
        )),
        thread(ME, "Today's digest: 6 new posts", &[INBOX, FORUMS]).msg(msg(
            SOURDOUGH,
            &[ME],
            day(1, 5, 0),
            "Starter smells like nail polish remover, help? (4 replies)\nRye at 80% \
                 hydration, my notes (2 replies)\nBest cheap Dutch oven? (11 replies)",
        )),
        thread(
            ME,
            "20% off winter vegetables this weekend",
            &[INBOX, PROMOTIONS],
        )
        .msg(msg(
            GROCER,
            &[ME],
            day(1, 10, 0),
            "Squash, leeks, kale and every root we could find: 20% off, Friday to \
                 Sunday.",
        )),
        thread(ME, "The winter sale starts now", &[INBOX, PROMOTIONS]).msg(msg(
            OUTFITTERS,
            &[ME],
            day(3, 9, 0),
            "Down jackets, wool layers and boots at up to 40% off, while they last.",
        )),
        thread(ME, "New in: waxed canvas", &[INBOX, PROMOTIONS]).msg(msg(
            OUTFITTERS,
            &[ME],
            day(9, 9, 0),
            "Bags and aprons in waxed canvas, made to last a lifetime.",
        )),
    ]
}
