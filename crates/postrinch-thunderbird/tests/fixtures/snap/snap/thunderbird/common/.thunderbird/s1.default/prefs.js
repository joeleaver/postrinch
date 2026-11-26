// Mozilla User Preferences
// An account whose server was renamed in Thunderbird's settings after it
// was made: realhostname and realuserName are what it connects to now.
user_pref("mail.account.account1.identities", "id1");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.accountmanager.accounts", "account1");
user_pref("mail.identity.id1.fullName", "Sam Jones");
user_pref("mail.identity.id1.useremail", "sam@acme.co");
user_pref("mail.server.server1.hostname", "old-mail.acme.co");
user_pref("mail.server.server1.realhostname", "mail.acme.co");
user_pref("mail.server.server1.realuserName", "sam.jones");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.userName", "sam");
user_pref("mail.smtp.defaultserver", "smtp1");
user_pref("mail.smtpserver.smtp1.hostname", "smtp.acme.co");
user_pref("mail.smtpserver.smtp1.try_ssl", 2);
user_pref("mail.smtpserver.smtp1.username", "sam.jones");
