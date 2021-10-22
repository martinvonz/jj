set send_human {0.1 0.3 1 0.05 1}
set timeout 2

proc expect_prompt {} {
    expect "$ "
}

proc run_command {cmd} {
    send -h "$cmd"
    send "\r"
    expect -timeout 5 "$ "
}

proc quit_and_dump_asciicast_path {} {
    set CTRLC \003
    set CTRLD \004
    set ESC \033

    send $CTRLD
    expect "asciinema: recording finished"
    sleep 1
    send $CTRLC
    expect -re "asciicast saved to (.+)$ESC.*\r" {
        send_user "$expect_out(1,string)\n"
    }
}
