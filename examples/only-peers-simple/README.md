# OnlyPeers (simple)

[Calimero]: https://www.calimero.network
[Node.js]: https://nodejs.org
[ReactJS]: https://react.dev
[Vite]: https://vitejs.dev
[pnpm]: https://pnpm.io

OnlyPeers (simple) is a [ReactJS][] project that demonstrates how to interact
with the [Calimero][] node using [`calimero-sdk`](https://github.com/calimero-network/core/tree/master/crates/sdk).
The project was created with [Vite][], a blazing-fast build tool that allows for
lightning-fast development and deployment of web applications.

# Getting Started

## Prerequisites

To run this project, you need to have [Node.js][] and [pnpm][] installed on your
computer.

## Installation

Clone the repository to your local machine:

```bash
git clone https://github.com/calimero-network/core
```

Navigate to the project directory:

```bash
cd examples/only-peers-simple
```

Install the project dependencies:

```bash
pnpm install
```

## Usage

To start the development server, run the following command:

```bash
pnpm dev
```

This will start a development server at [`http://localhost:3000`](http://localhost:3000).
Any changes you make to the code will be automatically hot-reloaded in the
browser.

To build the production-ready version of the application, run:

```bash
pnpm build
```

This will create a `dist` directory containing the compiled and optimized
version of the application.

To preview the production build, you can run:

```bash
pnpm preview
```

This will start a server that serves the production build at
[`http://localhost:5000`](http://localhost:5000).

# Acknowledgements

This project was created using the following technologies:

  - [ReactJS][]
  - [Vite][]
  - [`near-api-js`](https://www.npmjs.com/package/near-api-js)

# License

This project is licensed under the MIT License. See the [LICENSE](../../LICENSE.md)
file for details.
