from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_11_kind import (
    ConsentPageCreateBodyType11Kind,
)
from typing import cast

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_11_subject import (
        ConsentPageCreateBodyType11Subject,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType11")


@_attrs_define
class ConsentPageCreateBodyType11:
    """
    Attributes:
        kind (ConsentPageCreateBodyType11Kind):
        subject (ConsentPageCreateBodyType11Subject):
        return_to (str):
    """

    kind: ConsentPageCreateBodyType11Kind
    subject: "ConsentPageCreateBodyType11Subject"
    return_to: str

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_11_subject import (
            ConsentPageCreateBodyType11Subject,
        )

        kind = self.kind.value

        subject = self.subject.to_dict()

        return_to = self.return_to

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "subject": subject,
                "returnTo": return_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_11_subject import (
            ConsentPageCreateBodyType11Subject,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType11Kind(d.pop("kind"))

        subject = ConsentPageCreateBodyType11Subject.from_dict(d.pop("subject"))

        return_to = d.pop("returnTo")

        consent_page_create_body_type_11 = cls(
            kind=kind,
            subject=subject,
            return_to=return_to,
        )

        return consent_page_create_body_type_11
