from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_20_kind import (
    ConsentPageCreateBodyType20Kind,
)
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_20_subject import (
        ConsentPageCreateBodyType20Subject,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType20")


@_attrs_define
class ConsentPageCreateBodyType20:
    """
    Attributes:
        kind (ConsentPageCreateBodyType20Kind):
        return_to (str):
        subject (Union[Unset, ConsentPageCreateBodyType20Subject]):
    """

    kind: ConsentPageCreateBodyType20Kind
    return_to: str
    subject: Union[Unset, "ConsentPageCreateBodyType20Subject"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_20_subject import (
            ConsentPageCreateBodyType20Subject,
        )

        kind = self.kind.value

        return_to = self.return_to

        subject: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.subject, Unset):
            subject = self.subject.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "returnTo": return_to,
            }
        )
        if subject is not UNSET:
            field_dict["subject"] = subject

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_20_subject import (
            ConsentPageCreateBodyType20Subject,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType20Kind(d.pop("kind"))

        return_to = d.pop("returnTo")

        _subject = d.pop("subject", UNSET)
        subject: Union[Unset, ConsentPageCreateBodyType20Subject]
        if isinstance(_subject, Unset):
            subject = UNSET
        else:
            subject = ConsentPageCreateBodyType20Subject.from_dict(_subject)

        consent_page_create_body_type_20 = cls(
            kind=kind,
            return_to=return_to,
            subject=subject,
        )

        return consent_page_create_body_type_20
