import axios from "axios";

export default async function getApplicationIds() {
  return Object.keys((await axios.get("/admin-api/applications")).data.apps);
}
