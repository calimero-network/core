# Simple-Login

Simple-Login is a ReactJS project that demonstrates how to log in to a private shard with NEAR wallet using near-api-js. The project was created with vite, a blazing fast build tool that allows for lightning-fast development and deployment of web applications.
Getting Started
Prerequisites

To run this project, you need to have Node.js and npm installed on your computer.
Installation

Clone the repository to your local machine.

```
git clone https://github.com/calimero-is-near/calimero-sdk
```
Navigate to the project directory.

```
cd examples/simple-login
```
Install the project dependencies.

```
yarn install
```
Usage

To start the development server, run the following command:

```
yarn dev
```
This will start a development server at http://localhost:3000. Any changes you make to the code will be automatically hot-reloaded in the browser.

To build the production-ready version of the application, run:

```
yarn build
```
This will create a dist directory containing the compiled and optimized version of the application. To preview the production build, you can run:

```
yarn preview
```
This will start a server that serves the production build at http://localhost:5000.
Logging In with NEAR Wallet

To log in to a private shard with NEAR wallet, you need to have a NEAR account and some testnet NEAR tokens and access to Calimero Dashboard. You can get testnet NEAR tokens from the NEAR TestNet Wallet.

In the Simple-Login app, click on the "Log In" button in the top left corner of the screen.

You will be redirected to the NEAR Wallet page. Follow the instructions to log in with your NEAR account.

Once you have logged in with NEAR Wallet, you will be redirected back to the Simple-Login app. 

Acknowledgements

This project was created using the following technologies:

    ReactJS
    Vite
    near-api-js

License

This project is licensed under the MIT License. See the LICENSE file for details.
