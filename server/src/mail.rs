//! Sending email: over SMTP when configured, otherwise to the log (and, in
//! tests, to a captured list).

use std::sync::{Arc, Mutex};

use anyhow::Context;
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

/// A plain-text email.
#[derive(Clone, Debug)]
pub struct Email {
  pub to: String,
  pub subject: String,
  pub body: String,
}

/// Where emails go.
#[derive(Clone)]
pub enum Mailer {
  /// Delivered over SMTP from `from`, in the background.
  Smtp {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
  },
  /// Written to the log.
  Log,
  /// Kept in memory, for tests.
  Capture(Arc<Mutex<Vec<Email>>>),
}

impl Mailer {
  /// An SMTP mailer from a URL like `smtps://user:pass@host:465` and a
  /// `From` address.
  pub fn smtp(url: &str, from: &str) -> anyhow::Result<Self> {
    let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
      .context("parsing SMTP_URL")?
      .build();
    let from = from.parse().context("parsing MAIL_FROM")?;
    Ok(Mailer::Smtp { transport, from })
  }

  /// A mailer that records emails, and the list it records them in.
  pub fn capture() -> (Self, Arc<Mutex<Vec<Email>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    (Mailer::Capture(sent.clone()), sent)
  }

  /// Sends an email. SMTP delivery runs in the background so a slow mail
  /// server never holds up a request; failures are logged.
  pub fn send(&self, email: Email) {
    match self {
      Mailer::Smtp { transport, from } => {
        let message = match build_message(from, &email) {
          Ok(message) => message,
          Err(error) => {
            tracing::error!("couldn't build email to {}: {error:#}", email.to);
            return;
          }
        };
        let transport = transport.clone();
        tokio::spawn(async move {
          if let Err(error) = transport.send(message).await {
            tracing::error!("couldn't send email to {}: {error}", email.to);
          }
        });
      }
      Mailer::Log => {
        tracing::info!(
          "email to {}\nSubject: {}\n\n{}",
          email.to,
          email.subject,
          email.body
        );
      }
      Mailer::Capture(sent) => {
        if let Ok(mut sent) = sent.lock() {
          sent.push(email);
        }
      }
    }
  }
}

fn build_message(from: &Mailbox, email: &Email) -> anyhow::Result<Message> {
  Ok(
    Message::builder()
      .from(from.clone())
      .to(email.to.parse().context("parsing the recipient")?)
      .subject(&email.subject)
      .header(ContentType::TEXT_PLAIN)
      .body(email.body.clone())?,
  )
}

/// The email asking a new address to be verified.
pub fn verification_email(to: &str, username: &str, link: &str) -> Email {
  Email {
    to: to.to_string(),
    subject: "Verify your email for easl".into(),
    body: format!(
      "Hi {username},\n\n\
       Confirm this is your email address by opening this link:\n\n\
       {link}\n\n\
       The link works for 48 hours. If you didn't sign up for easl, you can \
       ignore this email.\n"
    ),
  }
}

/// The email with a password-reset link.
pub fn reset_email(to: &str, username: &str, link: &str) -> Email {
  Email {
    to: to.to_string(),
    subject: "Reset your easl password".into(),
    body: format!(
      "Hi {username},\n\n\
       Someone (hopefully you) asked to reset the password for your easl \
       account. Choose a new password here:\n\n\
       {link}\n\n\
       The link works for one hour. If you didn't ask for this, you can \
       ignore this email; your password hasn't changed.\n"
    ),
  }
}
