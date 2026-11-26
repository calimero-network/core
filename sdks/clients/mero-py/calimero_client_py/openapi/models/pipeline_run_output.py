from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="PipelineRunOutput")


@_attrs_define
class PipelineRunOutput:
    """A file a pipeline step made: its size and hash only to those who handle the app's files.

    Attributes:
        id (str):
        size (Union[Unset, int]):
        sha256 (Union[Unset, str]):
    """

    id: str
    size: Union[Unset, int] = UNSET
    sha256: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        size = self.size

        sha256 = self.sha256

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
            }
        )
        if size is not UNSET:
            field_dict["size"] = size
        if sha256 is not UNSET:
            field_dict["sha256"] = sha256

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        size = d.pop("size", UNSET)

        sha256 = d.pop("sha256", UNSET)

        pipeline_run_output = cls(
            id=id,
            size=size,
            sha256=sha256,
        )

        pipeline_run_output.additional_properties = d
        return pipeline_run_output

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
