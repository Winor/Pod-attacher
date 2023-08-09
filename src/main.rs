use inquire::Select;
use k8s_openapi::api::core::v1::{Pod, Namespace};
use kube::{Api, Client, api::{ListParams, AttachParams, TerminalSize}};
use tokio::{select, io::AsyncWriteExt};
use futures::{channel::mpsc::Sender, StreamExt};
use futures::SinkExt;
#[cfg(unix)] use tokio::signal;
use std::io::BufRead;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    loop {
        menu().await?;
    }
}

async fn menu() -> anyhow::Result<()> {
    let client = Client::try_default().await.unwrap();
    let ns_options = get_namespaces(&client).await?;
    let namespace = Select::new("Namespace", ns_options).prompt()?;
    let pod_options = get_pods(&client, namespace).await?;
    let pod = Select::new("Pod", pod_options.list).prompt()?;
    pod_shell(pod_options.pods, pod).await?; 
    Ok(())
}

async fn get_namespaces(client: &Client) -> anyhow::Result<Vec<String>> {
    let namespace: Api<Namespace> = Api::all(client.to_owned());
    let lp = ListParams::default();
    let nss = namespace.list(&lp).await?;
    Ok(nss.iter().map(|n|n.metadata.name.clone().unwrap_or_default()).collect())
}

struct Podata {
    pods: Api<Pod>,
    list: Vec<String>
}

async fn get_pods(client: &Client, namespace: String) -> anyhow::Result<Podata> {
    let pods: Api<Pod> = Api::namespaced(client.to_owned(), &namespace);
    let lp = ListParams::default();
    let nss = pods.list(&lp).await?;
    Ok(Podata {
        list: nss.iter().map(|n|n.metadata.name.clone().unwrap_or_default()).collect(),
        pods: pods
    })
}

async fn pod_shell(pods: Api<Pod>, name: String ) -> anyhow::Result<()> {
    let ap = AttachParams::interactive_tty();
    let mut attached = pods.exec(&name, vec!["/bin/bash"], &ap).await?;

    let mut stdin = tokio_util::io::ReaderStream::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();

    let mut output = tokio_util::io::ReaderStream::new(attached.stdout().unwrap());
    let mut input = attached.stdin().unwrap();

    let term_tx = attached.terminal_size().unwrap();

    let mut handle_terminal_size_handle = tokio::spawn(handle_terminal_size(term_tx));
    
    loop {
        select! {
            message = stdin.next() => {
                match message {
                    Some(Ok(message)) => {
                        let lines = message.lines().next();
                        if let Some(line) = lines {
                            input.write(line?.as_bytes()).await?;
                            input.write("\r".as_bytes()).await?;
                            input.flush().await?;
                        }
                    }
                    _ => {
                        break;
                    },
                }
            },
            message = output.next() => {
                match message {
                    Some(Ok(message)) => {
                        // println!("{:?}",str::from_utf8(&message).unwrap());
                        stdout.write(&message).await?;
                        stdout.flush().await?;
                    },
                    _ => {
                        break;
                    },
                }
            },
            result = &mut handle_terminal_size_handle => {
                match result {
                    Ok(_) => {
                        println!("End of terminal size stream");
                        break;
                    },
                    Err(e) => println!("Error getting terminal size: {e:?}")
                }
            },
        };
    }
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}


#[cfg(unix)]
// Send the new terminal size to channel when it change
async fn handle_terminal_size(mut channel: Sender<TerminalSize>) -> Result<(), anyhow::Error> {
    let (width, height) = crossterm::terminal::size()?;
    channel.send(TerminalSize { height, width }).await?;

    // create a stream to catch SIGWINCH signal
    let mut sig = signal::unix::signal(signal::unix::SignalKind::window_change())?;
    loop {
        if (sig.recv().await).is_none() {
            return Ok(());
        }

        let (width, height) = crossterm::terminal::size()?;
        channel.send(TerminalSize { height, width }).await?;
    }
}

#[cfg(windows)]
// We don't support window for terminal size change, we only send the initial size
async fn handle_terminal_size(mut channel: Sender<TerminalSize>) -> Result<(), anyhow::Error> {
    let (width, height) = crossterm::terminal::size()?;
    channel.send(TerminalSize { height, width }).await?;
    let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
    ctrl_c.recv().await;
    Ok(())
}