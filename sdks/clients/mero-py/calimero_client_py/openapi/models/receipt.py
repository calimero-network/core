from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="Receipt")


@_attrs_define
class Receipt:
    """The answering node's signature (receipt key, hex) over `(app, index, SHA-256(JCS(result)), node)`.

    Attributes:
        app (str):
        index (int):
        result_hash (str):
        node (int):
        key (str):
        signature (str):
    """

    app: str
    index: int
    result_hash: str
    node: int
    key: str
    signature: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app = self.app

        index = self.index

        result_hash = self.result_hash

        node = self.node

        key = self.key

        signature = self.signature

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "index": index,
                "resultHash": result_hash,
                "node": node,
                "key": key,
                "signature": signature,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app = d.pop("app")

        index = d.pop("index")

        result_hash = d.pop("resultHash")

        node = d.pop("node")

        key = d.pop("key")

        signature = d.pop("signature")

        receipt = cls(
            app=app,
            index=index,
            result_hash=result_hash,
            node=node,
            key=key,
            signature=signature,
        )

        receipt.additional_properties = d
        return receipt

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
