stage ("Builds") {
       node ('focal-fw') {
               stage ('Checkout') {
                       checkout scm
               }
               stage ('Install system packages') {
                       sh "sudo apt-get -y install build-essential mtools qemu-system-x86 libssl-dev pkg-config"
               }
               stage ('Install Rust') {
                       sh "nohup curl https://sh.rustup.rs -sSf | sh -s -- -y"
               }
               stage ('Run integration tests') {
                       sh "./run_integration_tests.sh"
               }
       }
}
