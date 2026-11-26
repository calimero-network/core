from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="WatchAdded")


@_attrs_define
class WatchAdded:
    """`POST /apps/{app}/watches`: the watch's id, which the stream's frames name, the app's last entry in the state the
    view read (a `last` to resume from) and the view's answer.

        Attributes:
            sub (str):
            index (int):
            answer (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    sub: str
    index: int
    answer: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        sub = self.sub

        index = self.index

        answer = self.answer

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "sub": sub,
                "index": index,
                "answer": answer,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        sub = d.pop("sub")

        index = d.pop("index")

        answer = d.pop("answer")

        watch_added = cls(
            sub=sub,
            index=index,
            answer=answer,
        )

        watch_added.additional_properties = d
        return watch_added

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
