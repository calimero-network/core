from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RestoreConsent")


@_attrs_define
class RestoreConsent:
    """An owner's or admin's step-up over `<app> / __restore_older / { backup, sha256, bundle, point }` (`POST
    /apps/{app}/restore-consent`; `point` is the restore point its open undo would revert, by its time, target and
    snapshot): the operator's restore carries it as `owner`, with the operator's signature naming the same bundle, to
    take a pinned instance (its record's `lineage`) from that backup onto that bundle, older than the release it runs,
    while that undo is open at a time no earlier than the upgrade and any upgrade logged since. Spent once.

        Attributes:
            account (str):
            consent (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    account: str
    consent: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        account = self.account

        consent = self.consent

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "account": account,
                "consent": consent,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        account = d.pop("account")

        consent = d.pop("consent")

        restore_consent = cls(
            account=account,
            consent=consent,
        )

        restore_consent.additional_properties = d
        return restore_consent

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
