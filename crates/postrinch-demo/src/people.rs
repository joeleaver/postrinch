//! Everyone in the demo store. Every one is made up, and every address is
//! at a domain reserved for examples (RFC 2606, RFC 6761): `example.com`,
//! `example.org`, `example.net` and names under `.example`. None can
//! belong to a real person or company.

use postrinch_model::{EmailAddress, Mailbox};

/// Someone who writes or is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Person {
    pub name: &'static str,
    pub address: &'static str,
}

impl Person {
    const fn new(name: &'static str, address: &'static str) -> Self {
        Self { name, address }
    }

    pub fn email(&self) -> EmailAddress {
        EmailAddress::parse(self.address).expect("the demo's addresses are well formed")
    }

    pub fn mailbox(&self) -> Mailbox {
        Mailbox::new(Some(self.name.to_owned()), self.email())
    }

    /// The part of the address after the `@`.
    pub fn domain(&self) -> &'static str {
        self.address
            .split_once('@')
            .map_or(self.address, |(_, d)| d)
    }
}

/// The person whose mail this is, as their two accounts know them.
pub const SAM_HOME: Person = Person::new("Sam Calloway", "sam@example.com");
pub const SAM_WORK: Person = Person::new("Sam Calloway", "sam@lindenworks.example");

// Family and friends.
pub const RUTH: Person = Person::new("Ruth Calloway", "ruth.calloway@example.net");
pub const JONAH: Person = Person::new("Jonah Pike", "jonah@example.org");
pub const MAYA: Person = Person::new("Maya Ortiz", "maya.ortiz@example.org");
pub const DANA: Person = Person::new("Dana Whitfield", "dana@whitfieldhomes.example");

// Shops, services and lists at home.
pub const PAPER_CRANE: Person = Person::new("Paper Crane Shop", "orders@papercrane.example");
pub const LIBRARY: Person = Person::new("Riverside Library", "notices@riversidelibrary.example");
pub const FIELD_NOTES: Person = Person::new("Field Notes Weekly", "letters@fieldnotes.example");
pub const RAIL: Person = Person::new("Northline Rail", "tickets@northlinerail.example");
pub const BANK: Person = Person::new("Cobalt Bank", "alerts@cobaltbank.example");
pub const WATER: Person = Person::new("Elm City Water", "billing@elmcitywater.example");
pub const TRAILMATES: Person = Person::new("Trailmates", "notify@trailmates.example");
pub const SOURDOUGH: Person = Person::new("Sourdough Circle", "digest@sourdoughcircle.example");
pub const GROCER: Person = Person::new("Greenleaf Grocer", "deals@greenleafgrocer.example");
pub const OUTFITTERS: Person = Person::new("Kestrel Outfitters", "hello@kestreloutfitters.example");
pub const CASA_AZUL: Person = Person::new("Casa Azul Stays", "bookings@casaazul.example");
pub const NURSERY: Person = Person::new("Hollow Oak Nursery", "orders@hollowoak.example");
pub const DENTIST: Person = Person::new("Brightside Dental", "care@brightsidedental.example");
pub const NEIGHBORS: Person = Person::new("Elm Row Neighbors", "hello@elmrow.example");

// The studio, Linden Works, and its clients.
pub const THEO: Person = Person::new("Theo Marsh", "theo@lindenworks.example");
pub const INES: Person = Person::new("Inês Duarte", "ines@lindenworks.example");
pub const CAREERS: Person = Person::new("Linden Works Careers", "careers@lindenworks.example");
pub const STUDIO_IT: Person = Person::new("Linden Works IT", "it@lindenworks.example");
pub const PRIYA: Person = Person::new("Priya Nand", "priya@harborcoffee.example");
pub const OLLIE: Person = Person::new("Ollie Grant", "ollie@harborcoffee.example");
pub const MARCUS: Person = Person::new("Marcus Bell", "marcus@fernhillbakery.example");
pub const LEDGERLY: Person = Person::new("Ledgerly", "billing@ledgerly.example");
pub const TRACKLIGHT: Person = Person::new("Tracklight", "notifications@tracklight.example");
pub const FIGMENT: Person = Person::new("Figment Fonts", "hello@figmentfonts.example");
pub const RIDGELINE: Person = Person::new("Ridgeline Print", "quotes@ridgelineprint.example");
pub const TYPE_TALK: Person = Person::new("Type Talk", "digest@typetalk.example");
pub const FOLIO: Person = Person::new("Folio", "notify@folio.example");

#[cfg(test)]
mod tests {
    use super::*;

    /// Domains RFC 2606 and RFC 6761 keep for examples.
    fn reserved(domain: &str) -> bool {
        ["example.com", "example.org", "example.net"].contains(&domain)
            || domain.ends_with(".example")
    }

    #[test]
    fn every_address_is_at_a_reserved_domain() {
        for person in [
            SAM_HOME,
            SAM_WORK,
            RUTH,
            JONAH,
            MAYA,
            DANA,
            PAPER_CRANE,
            LIBRARY,
            FIELD_NOTES,
            RAIL,
            BANK,
            WATER,
            TRAILMATES,
            SOURDOUGH,
            GROCER,
            OUTFITTERS,
            CASA_AZUL,
            NURSERY,
            DENTIST,
            NEIGHBORS,
            THEO,
            INES,
            CAREERS,
            STUDIO_IT,
            PRIYA,
            OLLIE,
            MARCUS,
            LEDGERLY,
            TRACKLIGHT,
            FIGMENT,
            RIDGELINE,
            TYPE_TALK,
            FOLIO,
        ] {
            assert!(
                reserved(person.domain()),
                "{} is not reserved",
                person.address
            );
            person.email();
        }
    }
}
