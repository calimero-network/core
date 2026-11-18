"""Generated client from WASM ABI schema"""

from typing import Dict, List, Any, Optional, Union
import json
from calimero import CloudClient, CloudError

from .types import *


class ABIClient:
    """Generated client for WASM ABI methods"""

    def __init__(self, client: CloudClient, app_id: str):
        """Initialize with a signed-in CloudClient and the app instance to call"""
        self.client = client
        self.app_id = app_id

    def init(self) -> Any:
        """
        init method from ABI

        Returns:
            Any: Return value
        """
        args = {}
        result = self.client.call(self.app_id, "init", args)["result"]
        return result

    def noop(self) -> None:
        """
        noop method from ABI

        Returns:
            None: Return value
        """
        args = {}
        result = self.client.call(self.app_id, "noop", args)["result"]
        return result

    def echo_bool(self, b: bool) -> bool:
        """
        echo_bool method from ABI

        Args:
            b (bool): Parameter

        Returns:
            bool: Return value
        """
        args = {"b": b}
        result = self.client.call(self.app_id, "echo_bool", args)["result"]
        return result

    def echo_i32(self, x: int) -> int:
        """
        echo_i32 method from ABI

        Args:
            x (int): Parameter

        Returns:
            int: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "echo_i32", args)["result"]
        return result

    def echo_i64(self, x: int) -> int:
        """
        echo_i64 method from ABI

        Args:
            x (int): Parameter

        Returns:
            int: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "echo_i64", args)["result"]
        return result

    def echo_u32(self, x: int) -> int:
        """
        echo_u32 method from ABI

        Args:
            x (int): Parameter

        Returns:
            int: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "echo_u32", args)["result"]
        return result

    def echo_u64(self, x: int) -> int:
        """
        echo_u64 method from ABI

        Args:
            x (int): Parameter

        Returns:
            int: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "echo_u64", args)["result"]
        return result

    def echo_f32(self, x: float) -> float:
        """
        echo_f32 method from ABI

        Args:
            x (float): Parameter

        Returns:
            float: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "echo_f32", args)["result"]
        return result

    def echo_f64(self, x: float) -> float:
        """
        echo_f64 method from ABI

        Args:
            x (float): Parameter

        Returns:
            float: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "echo_f64", args)["result"]
        return result

    def echo_string(self, s: str) -> str:
        """
        echo_string method from ABI

        Args:
            s (str): Parameter

        Returns:
            str: Return value
        """
        args = {"s": s}
        result = self.client.call(self.app_id, "echo_string", args)["result"]
        return result

    def echo_bytes(self, b: bytes) -> bytes:
        """
        echo_bytes method from ABI

        Args:
            b (bytes): Parameter

        Returns:
            bytes: Return value
        """
        args = {"b": b}
        result = self.client.call(self.app_id, "echo_bytes", args)["result"]
        return result

    def opt_u32(self, x: Optional[int]) -> Optional[int]:
        """
        opt_u32 method from ABI

        Args:
            x (Optional[int]): Parameter

        Returns:
            Optional[int]: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "opt_u32", args)["result"]
        return result

    def opt_string(self, x: Optional[str]) -> Optional[str]:
        """
        opt_string method from ABI

        Args:
            x (Optional[str]): Parameter

        Returns:
            Optional[str]: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "opt_string", args)["result"]
        return result

    def opt_record(self, p: Optional[Any]) -> Optional[Any]:
        """
        opt_record method from ABI

        Args:
            p (Optional[Any]): Parameter

        Returns:
            Optional[Any]: Return value
        """
        args = {"p": p}
        result = self.client.call(self.app_id, "opt_record", args)["result"]
        return result

    def opt_id(self, x: Optional[Any]) -> Optional[Any]:
        """
        opt_id method from ABI

        Args:
            x (Optional[Any]): Parameter

        Returns:
            Optional[Any]: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "opt_id", args)["result"]
        return result

    def list_u32(self, xs: List[int]) -> List[int]:
        """
        list_u32 method from ABI

        Args:
            xs (List[int]): Parameter

        Returns:
            List[int]: Return value
        """
        args = {"xs": xs}
        result = self.client.call(self.app_id, "list_u32", args)["result"]
        return result

    def list_strings(self, xs: List[str]) -> List[str]:
        """
        list_strings method from ABI

        Args:
            xs (List[str]): Parameter

        Returns:
            List[str]: Return value
        """
        args = {"xs": xs}
        result = self.client.call(self.app_id, "list_strings", args)["result"]
        return result

    def list_records(self, ps: List[Any]) -> List[Any]:
        """
        list_records method from ABI

        Args:
            ps (List[Any]): Parameter

        Returns:
            List[Any]: Return value
        """
        args = {"ps": ps}
        result = self.client.call(self.app_id, "list_records", args)["result"]
        return result

    def list_ids(self, xs: List[Any]) -> List[Any]:
        """
        list_ids method from ABI

        Args:
            xs (List[Any]): Parameter

        Returns:
            List[Any]: Return value
        """
        args = {"xs": xs}
        result = self.client.call(self.app_id, "list_ids", args)["result"]
        return result

    def map_u32(self, m: Dict[str, int]) -> Dict[str, int]:
        """
        map_u32 method from ABI

        Args:
            m (Dict[str, int]): Parameter

        Returns:
            Dict[str, int]: Return value
        """
        args = {"m": m}
        result = self.client.call(self.app_id, "map_u32", args)["result"]
        return result

    def map_list_u32(self, m: Dict[str, List[int]]) -> Dict[str, List[int]]:
        """
        map_list_u32 method from ABI

        Args:
            m (Dict[str, List[int]]): Parameter

        Returns:
            Dict[str, List[int]]: Return value
        """
        args = {"m": m}
        result = self.client.call(self.app_id, "map_list_u32", args)["result"]
        return result

    def map_record(self, m: Dict[str, Any]) -> Dict[str, Any]:
        """
        map_record method from ABI

        Args:
            m (Dict[str, Any]): Parameter

        Returns:
            Dict[str, Any]: Return value
        """
        args = {"m": m}
        result = self.client.call(self.app_id, "map_record", args)["result"]
        return result

    def make_person(self, p: Any) -> Any:
        """
        make_person method from ABI

        Args:
            p (Any): Parameter

        Returns:
            Any: Return value
        """
        args = {"p": p}
        result = self.client.call(self.app_id, "make_person", args)["result"]
        return result

    def profile_roundtrip(self, p: Any) -> Any:
        """
        profile_roundtrip method from ABI

        Args:
            p (Any): Parameter

        Returns:
            Any: Return value
        """
        args = {"p": p}
        result = self.client.call(self.app_id, "profile_roundtrip", args)["result"]
        return result

    def act(self, a: Any) -> int:
        """
        act method from ABI

        Args:
            a (Any): Parameter

        Returns:
            int: Return value
        """
        args = {"a": a}
        result = self.client.call(self.app_id, "act", args)["result"]
        return result

    def roundtrip_id(self, x: Any) -> Any:
        """
        roundtrip_id method from ABI

        Args:
            x (Any): Parameter

        Returns:
            Any: Return value
        """
        args = {"x": x}
        result = self.client.call(self.app_id, "roundtrip_id", args)["result"]
        return result

    def roundtrip_hash(self, h: Any) -> Any:
        """
        roundtrip_hash method from ABI

        Args:
            h (Any): Parameter

        Returns:
            Any: Return value
        """
        args = {"h": h}
        result = self.client.call(self.app_id, "roundtrip_hash", args)["result"]
        return result

    def may_fail(self, flag: bool) -> int:
        """
        may_fail method from ABI

        Args:
            flag (bool): Parameter

        Returns:
            int: Return value
        """
        args = {"flag": flag}
        result = self.client.call(self.app_id, "may_fail", args)["result"]
        return result

    def find_person(self, name: str) -> Any:
        """
        find_person method from ABI

        Args:
            name (str): Parameter

        Returns:
            Any: Return value
        """
        args = {"name": name}
        result = self.client.call(self.app_id, "find_person", args)["result"]
        return result
