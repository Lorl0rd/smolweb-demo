async function toggle_led2() {
    let response = await fetch("/toggle_led/2");
    let response_text = await response.text();
    document.getElementById("led2Label").innerText = response_text;
}
