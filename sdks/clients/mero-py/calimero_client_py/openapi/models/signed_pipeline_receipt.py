from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SignedPipelineReceipt")


@_attrs_define
class SignedPipelineReceipt:
    """`GET /apps/{app}/pipeline-runs/{run}/receipt`: a pipeline run's receipt
    (`calimero_client::receipt::PipelineReceipt`) with the signature of the answering node's receipt key over it.

        Attributes:
            receipt (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            node (int):
            signature (str):
    """

    receipt: Any
    node: int
    signature: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        receipt = self.receipt

        node = self.node

        signature = self.signature

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "receipt": receipt,
                "node": node,
                "signature": signature,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        receipt = d.pop("receipt")

        node = d.pop("node")

        signature = d.pop("signature")

        signed_pipeline_receipt = cls(
            receipt=receipt,
            node=node,
            signature=signature,
        )

        signed_pipeline_receipt.additional_properties = d
        return signed_pipeline_receipt

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
